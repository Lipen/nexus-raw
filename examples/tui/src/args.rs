//! Command line arguments for `nxr-tui`.

use anyhow::bail;

const USAGE: &str = r#"nxr-tui: a terminal browser over a Nexus raw repository

usage:
  nxr-tui [options] BASE_URL...

arguments:
  BASE_URL...          one or more server root URLs, e.g. http://127.0.0.1:8099/

options:
  -u, --user USER:PASS  credentials, curl style; env fallback: NXR_AUTH or NXR_USERNAME + NXR_PASSWORD
      --smoke           headless mode: run the same browse-and-download flow and print the summary
  -h, --help            print this help

keys:
  up/down or k/j        move the selection
  enter                 drill down, or download the selected version subtree
  esc                   go back one screen
  q or ctrl-c           quit
"#;

/// Parsed command line.
#[derive(Debug, Clone)]
pub struct Args {
    /// Server root URLs, each serving the service REST API and at least one raw repository.
    pub bases: Vec<String>,
    /// Credentials as `user:pass`, curl style.
    pub user: Option<String>,
    /// Headless smoke mode: the same browse-and-download flow without the TUI.
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
                "--smoke" => smoke = true,
                _ if arg.starts_with('-') && arg != "-" => bail!("unknown flag: {arg}"),
                _ => bases.push(arg),
            }
        }
        if bases.is_empty() {
            bail!("at least one server base URL is required");
        }
        Ok(Some(Self { bases, user, smoke }))
    }
}
