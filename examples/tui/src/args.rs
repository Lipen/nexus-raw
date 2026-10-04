//! Command line arguments for `nxr-tui`.

use anyhow::bail;

const USAGE: &str = r#"nxr-tui: a terminal browser over a Nexus raw repository

usage:
  nxr-tui [options] BASE_URL...

arguments:
  BASE_URL...          one or more server root URLs, e.g. http://127.0.0.1:8099/

options:
  -u, --user USER:PASS  credentials, curl style; env fallback: NXR_AUTH or NXR_USERNAME + NXR_PASSWORD
      --all-formats     let repositories of every format be opened, not only raw
      --smoke           headless mode: list the tree, download a subtree, print the summary
  -h, --help            print this help

keys:
  up/down or k/j        move the selection
  enter                 open a folder, or download the selected file
  d                     download the selected entry: a folder subtree or a single file
  esc or backspace      go up one level
  q or ctrl-c           quit
"#;

/// Parsed command line.
#[derive(Debug, Clone)]
pub struct Args {
    /// Server root URLs, each serving the service REST API and at least one raw repository.
    pub bases: Vec<String>,
    /// Credentials as `user:pass`, curl style.
    pub user: Option<String>,
    /// Let repositories of every format be opened, not only raw.
    pub all_formats: bool,
    /// Headless smoke mode: the browse-and-download flow without the TUI.
    pub smoke: bool,
}

impl Args {
    /// Parses arguments.
    /// `Ok(None)` means the help text was printed and the process should exit successfully.
    ///
    /// # Errors
    ///
    /// Returns an error on an unknown flag, a missing `-u` value, or an empty server list.
    pub fn parse<I>(it: I) -> anyhow::Result<Option<Self>>
    where
        I: IntoIterator<Item = String>,
    {
        let mut items = it.into_iter();
        let mut bases = Vec::new();
        let mut user = None;
        let mut all_formats = false;
        let mut smoke = false;
        while let Some(arg) = items.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    print!("{USAGE}");
                    return Ok(None);
                }
                "-u" | "--user" => {
                    let Some(value) = items.next() else {
                        bail!("-u expects a value: user:pass");
                    };
                    user = Some(value);
                }
                "--all-formats" => all_formats = true,
                "--smoke" => smoke = true,
                _ if arg.starts_with('-') && arg != "-" => bail!("unknown flag: {arg}"),
                _ => bases.push(arg),
            }
        }
        if bases.is_empty() {
            bail!("at least one server base URL is required");
        }
        Ok(Some(Self {
            bases,
            user,
            all_formats,
            smoke,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<Args> {
        Args::parse(args.iter().map(ToString::to_string)).map(Option::unwrap)
    }

    #[test]
    fn parses_bases_user_and_flags() -> anyhow::Result<()> {
        let args = parse(&[
            "http://a/",
            "http://b/",
            "-u",
            "u:p",
            "--all-formats",
            "--smoke",
        ])?;
        assert_eq!(args.bases, vec!["http://a/", "http://b/"]);
        assert_eq!(args.user.as_deref(), Some("u:p"));
        assert!(args.all_formats);
        assert!(args.smoke);
        Ok(())
    }

    #[test]
    fn defaults_are_off() -> anyhow::Result<()> {
        let args = parse(&["http://a/"])?;
        assert!(!args.all_formats);
        assert!(!args.smoke);
        assert!(args.user.is_none());
        Ok(())
    }

    #[test]
    fn refuses_unknown_flags_and_empty_bases() {
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&[]).is_err());
        assert!(parse(&["-u"]).is_err());
    }
}
