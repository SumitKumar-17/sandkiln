//! Layered value resolution: environment variable, then config file, then
//! the registry default. Every problem is accumulated rather than panicking
//! on the first, so one startup attempt reports everything that is wrong.

use super::settings::{self, Setting};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Default,
    File,
    Env,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Default => "default",
            Origin::File => "file",
            Origin::Env => "env",
        }
    }
}

pub struct Resolver {
    env: HashMap<String, String>,
    file: BTreeMap<String, String>,
    pub errors: Vec<String>,
    pub origins: BTreeMap<&'static str, Origin>,
}

impl Resolver {
    pub fn new(env: HashMap<String, String>, file: BTreeMap<String, String>) -> Self {
        Self { env, file, errors: Vec::new(), origins: BTreeMap::new() }
    }

    fn setting(key: &str) -> &'static Setting {
        settings::find(key).unwrap_or_else(|| panic!("BUG: setting `{key}` is not in the registry"))
    }

    /// Raw value and where it came from. A set-but-empty environment
    /// variable counts as unset, so `export SANDKILN_X=` clears a file value
    /// instead of silently leaving it.
    pub fn raw(&mut self, key: &str) -> Option<String> {
        let setting = Self::setting(key);
        let (value, origin) = match self.env.get(&setting.env_name()).filter(|v| !v.is_empty()) {
            Some(v) => (Some(v.clone()), Origin::Env),
            None => match self.file.get(key) {
                Some(v) => (Some(v.clone()), Origin::File),
                None => (setting.default.map(str::to_string), Origin::Default),
            },
        };
        self.origins.insert(setting.key, origin);
        value
    }

    pub fn string(&mut self, key: &str) -> String {
        self.raw(key).unwrap_or_default()
    }

    pub fn opt_string(&mut self, key: &str) -> Option<String> {
        self.raw(key)
    }

    pub fn number<T: FromStr + Default>(&mut self, key: &str) -> T {
        match self.raw(key) {
            None => T::default(),
            Some(v) => v.trim().parse().unwrap_or_else(|_| {
                self.errors.push(format!("{} must be a number, got `{v}`", Self::setting(key).env_name()));
                T::default()
            }),
        }
    }

    /// `None` when unset or 0: the shared "0 disables it" convention.
    pub fn opt_secs(&mut self, key: &str) -> Option<std::time::Duration> {
        let v = self.raw(key)?;
        let secs: u64 = v.trim().parse().unwrap_or_else(|_| {
            self.errors.push(format!("{} must be a number of seconds, got `{v}`", Self::setting(key).env_name()));
            0
        });
        (secs > 0).then(|| std::time::Duration::from_secs(secs))
    }

    pub fn flag(&mut self, key: &str) -> bool {
        matches!(self.raw(key).as_deref(), Some(v) if v.eq_ignore_ascii_case("true") || v == "1")
    }

    pub fn path(&mut self, key: &str) -> PathBuf {
        let raw = self.string(key);
        self.expand_home(&raw)
    }

    pub fn opt_path(&mut self, key: &str) -> Option<PathBuf> {
        self.raw(key).map(|raw| self.expand_home(&raw))
    }

    fn expand_home(&mut self, raw: &str) -> PathBuf {
        match raw.strip_prefix("~/") {
            None => PathBuf::from(raw),
            Some(rest) => match self.env.get("HOME") {
                Some(home) => Path::new(home).join(rest),
                None => {
                    self.errors.push(format!("HOME is not set, so `{raw}` cannot be expanded"));
                    PathBuf::from(rest)
                }
            },
        }
    }

    pub fn error(&mut self, message: String) {
        self.errors.push(message);
    }
}

/// Parses a config file body into `key -> string value`, rejecting unknown
/// keys (with a nearest-key hint) and non-scalar values.
pub fn parse_file(contents: &str) -> Result<BTreeMap<String, String>, Vec<String>> {
    let table: toml::Table = contents.parse().map_err(|e: toml::de::Error| vec![format!("invalid TOML: {e}")])?;
    let mut out = BTreeMap::new();
    let mut errors = Vec::new();
    for (key, value) in table {
        if settings::find(&key).is_none() {
            let hint = nearest(&key).map(|k| format!(" (did you mean `{k}`?)")).unwrap_or_default();
            errors.push(format!("unknown key `{key}`{hint}"));
            continue;
        }
        let text = match value {
            toml::Value::String(s) => s,
            toml::Value::Integer(i) => i.to_string(),
            toml::Value::Boolean(b) => b.to_string(),
            other => {
                errors.push(format!("`{key}` must be a string, number or boolean, got {}", other.type_str()));
                continue;
            }
        };
        out.insert(key, text);
    }
    if errors.is_empty() { Ok(out) } else { Err(errors) }
}

fn nearest(key: &str) -> Option<&'static str> {
    settings::SETTINGS.iter().map(|s| (edit_distance(key, s.key), s.key)).filter(|(d, _)| *d <= 3).min().map(|(_, k)| k)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur.push(sub.min(prev[j] + 1).min(cur[j - 1] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(env: &[(&str, &str)], file: &[(&str, &str)]) -> Resolver {
        Resolver::new(
            env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            file.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        )
    }

    #[test]
    fn env_beats_file_beats_default() {
        let mut r = resolver(&[("SANDKILN_VCPU_COUNT", "8")], &[("vcpu_count", "4"), ("mem_size_mib", "1024")]);
        assert_eq!(r.number::<u8>("vcpu_count"), 8);
        assert_eq!(r.number::<u32>("mem_size_mib"), 1024);
        assert_eq!(r.number::<u32>("max_mem_size_mib"), 16384);
        assert_eq!(r.origins["vcpu_count"], Origin::Env);
        assert_eq!(r.origins["mem_size_mib"], Origin::File);
        assert_eq!(r.origins["max_mem_size_mib"], Origin::Default);
    }

    #[test]
    fn empty_env_var_falls_through_to_the_file() {
        let mut r = resolver(&[("SANDKILN_VCPU_COUNT", "")], &[("vcpu_count", "4")]);
        assert_eq!(r.number::<u8>("vcpu_count"), 4);
    }

    #[test]
    fn bad_numbers_are_accumulated_not_panicked() {
        let mut r = resolver(&[("SANDKILN_VCPU_COUNT", "lots"), ("SANDKILN_TAP_POOL_SIZE", "-1")], &[]);
        r.number::<u8>("vcpu_count");
        r.number::<u32>("tap_pool_size");
        assert_eq!(r.errors.len(), 2);
        assert!(r.errors[0].contains("SANDKILN_VCPU_COUNT") && r.errors[0].contains("lots"));
    }

    #[test]
    fn zero_or_unset_seconds_disable_a_timeout() {
        let mut r = resolver(&[("SANDKILN_IDLE_TIMEOUT_SECS", "0")], &[]);
        assert_eq!(r.opt_secs("idle_timeout_secs"), None);
        assert_eq!(r.opt_secs("archive_timeout_secs"), None);
        let mut r = resolver(&[("SANDKILN_IDLE_TIMEOUT_SECS", "90")], &[]);
        assert_eq!(r.opt_secs("idle_timeout_secs"), Some(std::time::Duration::from_secs(90)));
    }

    #[test]
    fn tilde_expands_against_home_and_errors_without_it() {
        let mut r = resolver(&[("HOME", "/home/u")], &[]);
        assert_eq!(r.path("state_dir"), PathBuf::from("/home/u/sandkiln-tools/state"));
        let mut r = resolver(&[], &[]);
        r.path("state_dir");
        assert!(r.errors[0].contains("HOME"));
    }

    #[test]
    fn file_parse_accepts_scalars_and_stringifies_them() {
        let f = parse_file("vcpu_count = 4\nlog_format = \"json\"\njailer_enabled = true\n").unwrap();
        assert_eq!(f["vcpu_count"], "4");
        assert_eq!(f["log_format"], "json");
        assert_eq!(f["jailer_enabled"], "true");
    }

    #[test]
    fn file_parse_rejects_unknown_keys_with_a_hint() {
        let errs = parse_file("vcpu_cuont = 4\n").unwrap_err();
        assert!(errs[0].contains("unknown key `vcpu_cuont`") && errs[0].contains("did you mean `vcpu_count`"), "{errs:?}");
    }

    #[test]
    fn file_parse_rejects_nested_values_and_bad_toml() {
        assert!(parse_file("vcpu_count = [1, 2]\n").unwrap_err()[0].contains("must be a string, number or boolean"));
        assert!(parse_file("this is not toml").unwrap_err()[0].contains("invalid TOML"));
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
    }
}
