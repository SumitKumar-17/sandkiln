//! The environment the agent and everything it spawns starts with.
//!
//! It must match what a systemd-run agent had, or the same command behaves
//! differently across the two image kinds. systemd's manager imported the
//! locale configuration (`/etc/locale.conf`, or Debian and Ubuntu's
//! `/etc/default/locale`) into every service; PID 1 here does the same
//! instead of hard-coding a locale the image may not have.

/// Used when the image carries no locale configuration, as the Firecracker
/// CI image does not.
const DEFAULT_LANG: (&str, &str) = ("LANG", "C.UTF-8");

const PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Locale files in precedence order: systemd's own first.
const LOCALE_FILES: &[&str] = &["/etc/locale.conf", "/etc/default/locale"];

/// The locale variables systemd forwards to services.
fn is_locale_key(key: &str) -> bool {
    key == "LANG" || key == "LANGUAGE" || key.starts_with("LC_")
}

/// `KEY=value` lines of a locale file: comments skipped, an optional
/// `export` prefix and one pair of surrounding quotes removed, and any key
/// that is not a locale variable ignored.
pub(super) fn parse_locale(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            let value = value.trim();
            let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))).unwrap_or(value);
            (is_locale_key(key) && !value.is_empty()).then(|| (key.to_string(), value.to_string()))
        })
        .collect()
}

/// The full variable list: `PATH`, plus the locale variables of the first
/// locale file that defines any, else the default `LANG`.
pub(super) fn build(locale_file_contents: impl IntoIterator<Item = String>) -> Vec<(String, String)> {
    let locale = locale_file_contents.into_iter().map(|c| parse_locale(&c)).find(|vars| !vars.is_empty());
    let mut vars = locale.unwrap_or_else(|| vec![(DEFAULT_LANG.0.to_string(), DEFAULT_LANG.1.to_string())]);
    vars.push(("PATH".to_string(), PATH.to_string()));
    vars
}

/// Replaces the kernel-provided environment (`HOME=/`, `TERM=linux`) with
/// the list above, so exec and shells see what they saw under systemd.
pub(super) fn apply() {
    for (key, _) in std::env::vars_os().collect::<Vec<_>>() {
        std::env::remove_var(key);
    }
    let files = LOCALE_FILES.iter().filter_map(|path| std::fs::read_to_string(path).ok());
    for (key, value) in build(files) {
        std::env::set_var(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
        vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn no_locale_file_falls_back_to_the_default_lang_and_a_path() {
        let vars = build(Vec::new());
        assert_eq!(get(&vars, "LANG"), Some("C.UTF-8"));
        assert!(get(&vars, "PATH").unwrap().contains("/usr/local/bin"));
    }

    #[test]
    fn lang_comes_from_the_image_when_it_has_one() {
        let vars = build(vec!["LANG=en_US.UTF-8\n".to_string()]);
        assert_eq!(get(&vars, "LANG"), Some("en_US.UTF-8"));
    }

    #[test]
    fn quotes_comments_export_and_non_locale_keys_are_handled() {
        let parsed = parse_locale("# c\nexport LANG=\"de_DE.UTF-8\"\nLC_TIME='fr_FR.UTF-8'\nLANGUAGE=en\nPATH=/evil\nFOO=bar\nLC_ALL=\n");
        assert_eq!(
            parsed,
            vec![
                ("LANG".to_string(), "de_DE.UTF-8".to_string()),
                ("LC_TIME".to_string(), "fr_FR.UTF-8".to_string()),
                ("LANGUAGE".to_string(), "en".to_string()),
            ]
        );
    }

    #[test]
    fn the_first_file_that_defines_a_locale_wins_and_an_empty_one_is_skipped() {
        let vars = build(vec!["# nothing here\n".to_string(), "LANG=C.UTF-8\nLC_CTYPE=en_US.UTF-8\n".to_string(), "LANG=ignored\n".to_string()]);
        assert_eq!(get(&vars, "LANG"), Some("C.UTF-8"));
        assert_eq!(get(&vars, "LC_CTYPE"), Some("en_US.UTF-8"));
    }

    #[test]
    fn nothing_systemd_private_is_set() {
        let vars = build(Vec::new());
        assert!(vars.iter().all(|(k, _)| !k.contains("SYSTEMD") && k != "INVOCATION_ID" && k != "JOURNAL_STREAM"));
    }
}
