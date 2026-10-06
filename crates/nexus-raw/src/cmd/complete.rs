//! The `complete` command: a shell completion script generated from the running clap tree.

use std::io::{self, Write};

use clap::CommandFactory as _;
use clap_complete::{generate, Shell};

use nexus_raw_core::Error;

use crate::Cli;

/// Print the completion script for `shell` to stdout.
/// The script is generated from the same clap tree the binary dispatches on,
/// so it names exactly the commands and flags this build has.
pub(crate) fn complete(shell: &str) -> Result<(), Error> {
    let shell = match shell {
        "bash" => Shell::Bash,
        "zsh" => Shell::Zsh,
        "fish" => Shell::Fish,
        "powershell" => Shell::PowerShell,
        other => {
            return Err(Error::Misuse(format!(
                "unknown shell: {other} (use bash, zsh, fish or powershell)"
            )));
        }
    };
    // The script is built in memory first: the generators unwrap every write,
    // so a consumer that hangs up early (`nxr complete bash | head`) would panic inside them.
    // The one controlled write fails quiet, the way `--help` prints: the script is
    // best-effort output to stdout, and a closed pipe is the reader's decision.
    let mut script = Vec::new();
    let mut cmd = Cli::command();
    let bin = cmd.get_name().to_owned();
    generate(shell, &mut cmd, bin, &mut script);
    let mut stdout = io::stdout();
    let _ = stdout.write_all(&script).and_then(|()| stdout.flush());
    Ok(())
}
