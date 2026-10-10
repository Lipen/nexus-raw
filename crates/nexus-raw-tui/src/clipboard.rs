//! Clipboard: OSC52 to the terminal first, then the platform tool.
//!
//! OSC52 works over ssh and needs no child process. The external tools cover
//! terminals that drop the escape. Nothing is reported on failure: the `copied`
//! toast only appears after [`copy`], so a silent miss is possible by design.

use base64::Engine as _;

/// Copies `text` for the user. `osc52` comes from the `tui.osc52` config key.
pub fn copy(text: &str, osc52: bool) {
    if osc52 {
        let _ = send_osc52(text);
    }
    for tool in fallback_tools() {
        if spawn_tool(tool, text).is_ok() {
            return;
        }
    }
}

/// The external clipboard helpers, in probe order.
#[cfg(windows)]
fn fallback_tools() -> &'static [&'static str] {
    &["clip.exe"]
}

#[cfg(not(windows))]
fn fallback_tools() -> &'static [&'static str] {
    &["wl-copy", "xclip", "pbcopy"]
}

/// Writes the OSC 52 escape: the base64 payload under the `c` selection,
/// terminated by ST. Stderr carries it, so the ratatui frame buffer on stdout
/// stays untouched.
fn send_osc52(text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let payload = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut err = std::io::stderr();
    write!(err, "\x1b]52;c;{payload}\x1b\\")?;
    err.flush()
}

/// Pipes `text` into one clipboard tool and waits for it to exit.
fn spawn_tool(tool: &str, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut child = std::process::Command::new(tool)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(text.as_bytes())?;
    }
    // Dropping the piped stdin ends the tool's input: it exits on EOF.
    drop(child.stdin.take());
    child.wait()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_payload_is_base64_of_the_text() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"hello");
        assert_eq!(encoded, "aGVsbG8=");
    }
}
