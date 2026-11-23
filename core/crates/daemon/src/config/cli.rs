//! `sandkilnd`'s own flags. Hand-parsed: there are six of them, and a
//! dependency for that would cost more than it saves.

use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Run,
    CheckConfig,
    PrintConfig,
    ConfigReference,
    Help,
    Version,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Cli {
    pub command: Command,
    pub config_file: Option<PathBuf>,
}

pub const USAGE: &str = "\
sandkilnd: the sandkiln daemon

USAGE:
    sandkilnd [--config <file>] [COMMAND]

OPTIONS:
    --config <file>      Config file (TOML). Without it: $SANDKILN_CONFIG, ./sandkiln.toml,
                         then ~/.config/sandkiln/sandkiln.toml. Environment variables
                         (SANDKILN_*) always override the file.

COMMANDS (default: run the daemon):
    --check-config       Validate configuration and exit 0 or 2.
    --print-config       Print every effective setting and where it came from.
    --config-reference   Print the settings reference as a Markdown table.
    --help, --version
";

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Cli, String> {
    let mut cli = Cli { command: Command::Run, config_file: None };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        match flag.as_str() {
            "--config" => {
                let value = inline.or_else(|| args.next()).ok_or("--config needs a file path")?;
                cli.config_file = Some(PathBuf::from(value));
            }
            "--check-config" => cli.command = Command::CheckConfig,
            "--print-config" => cli.command = Command::PrintConfig,
            "--config-reference" => cli.command = Command::ConfigReference,
            "--help" | "-h" => cli.command = Command::Help,
            "--version" | "-V" => cli.command = Command::Version,
            other => return Err(format!("unknown argument `{other}` (see --help)")),
        }
    }
    Ok(cli)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cli, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_args_runs_the_daemon() {
        assert_eq!(p(&[]).unwrap(), Cli { command: Command::Run, config_file: None });
    }

    #[test]
    fn config_accepts_both_spellings() {
        assert_eq!(p(&["--config", "a.toml"]).unwrap().config_file, Some(PathBuf::from("a.toml")));
        assert_eq!(p(&["--config=a.toml"]).unwrap().config_file, Some(PathBuf::from("a.toml")));
    }

    #[test]
    fn commands_combine_with_a_config_file() {
        let cli = p(&["--check-config", "--config", "x.toml"]).unwrap();
        assert_eq!(cli.command, Command::CheckConfig);
        assert_eq!(cli.config_file, Some(PathBuf::from("x.toml")));
    }

    #[test]
    fn missing_value_and_unknown_flags_are_errors() {
        assert!(p(&["--config"]).is_err());
        assert!(p(&["--bogus"]).unwrap_err().contains("--bogus"));
    }
}
