//! The documentation surface against the CLI it documents.
//!
//! Every `nxr …` command line in every markdown file of the repository (the docs
//! site, the crate and example READMEs, the root READMEs) must name flags the
//! binary accepts, and `docs/reference/cli.md` must carry a section per command
//! (and per `service` subcommand). A flag renamed in clap therefore breaks this
//! test instead of leaving a command that no longer parses in the docs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const NXR: &str = env!("CARGO_BIN_EXE_nxr");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `--flags` and subcommand ids of `nxr <path> --help`; empty when the command refuses.
fn help_of(path: &[String]) -> String {
    let out = Command::new(NXR)
        .args(path)
        .arg("--help")
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD")
        .output()
        .expect("nxr runs");
    assert!(
        out.status.success(),
        "nxr {} --help failed: {}",
        path.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn flags_in(help: &str) -> BTreeSet<String> {
    help.split_whitespace()
        .filter(|t| t.starts_with("--") && t.len() > 2)
        .map(|t| {
            t.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .trim_end_matches('-')
                .to_owned()
        })
        .collect()
}

/// The subcommand ids listed under `Commands:` in a help text.
fn commands_of(help: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut inside = false;
    for line in help.lines() {
        if line.trim() == "Commands:" {
            inside = true;
            continue;
        }
        if inside {
            if line.trim().is_empty() {
                break;
            }
            let Some(rest) = line.strip_prefix("  ") else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if !name.is_empty() && name != "help" {
                out.insert(name);
            }
        }
    }
    out
}

/// Every markdown file of the repository, minus the build and private trees.
fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("dir reads") {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if matches!(name, "target" | ".git" | ".dev" | "node_modules" | "pkg") {
                continue;
            }
            markdown_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Every `nxr ...` command line in the docs parses against the binary's help.
#[test]
fn documented_command_lines_use_flags_the_cli_accepts() {
    let root = repo_root();
    let root_help = help_of(&[]);
    let globals = flags_in(&root_help);
    let top = commands_of(&root_help);
    let mut files = Vec::new();
    markdown_files(&root, &mut files);
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for file in &files {
        let text = std::fs::read_to_string(file).expect("doc reads");
        for (n, line) in text.lines().enumerate() {
            let trimmed = line.trim_start_matches(|c: char| c == '$' || c.is_whitespace());
            let Some(rest) = trimmed.strip_prefix("nxr ") else {
                continue;
            };
            // Prompts, placeholders and tables are prose, not command lines.
            if rest.starts_with(['[', '{', '|', '<']) {
                continue;
            }
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            // The command is the first token the binary knows: a global flag
            // carries a value (`--workers 0 head …`), and `-R alias` an alias,
            // so the first word is not necessarily the command.
            let Some(cmd) = tokens.iter().find(|t| top.contains(**t)) else {
                continue;
            };
            checked += 1;
            let mut path = vec![(*cmd).to_owned()];
            if *cmd == "service" || *cmd == "channel" {
                let sub = tokens
                    .iter()
                    .skip_while(|t| *t != cmd)
                    .skip(1)
                    .find(|t| !t.starts_with('-'));
                if let Some(sub) = sub {
                    path.push((*sub).trim_matches('"').to_owned());
                }
            }
            let known = flags_in(&help_of(&path))
                .union(&globals)
                .cloned()
                .collect::<BTreeSet<_>>();
            for token in tokens {
                if !token.starts_with("--") || token.len() < 3 {
                    continue;
                }
                let flag: String = token
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                if !known.contains(&flag) {
                    failures.push(format!(
                        "{}:{}: nxr {} does not accept {flag}",
                        file.strip_prefix(&root).unwrap_or(file).display(),
                        n + 1,
                        path.join(" ")
                    ));
                }
            }
        }
    }
    // A walk or a parser that silently matches nothing would pass forever.
    assert!(
        files.len() >= 40 && checked >= 100,
        "the audit covered {} files and {checked} command lines: the walk or the parser regressed",
        files.len()
    );
    assert!(
        failures.is_empty(),
        "documented flags drift from the CLI:\n{}",
        failures.join("\n")
    );
}

/// Every public method of the `Nxr` facade is named in the API reference: a new
/// method cannot land undocumented, and a rename cannot leave a stale row alone.
#[test]
fn the_api_reference_names_every_facade_method() {
    let root = repo_root();
    let source = std::fs::read_to_string(root.join("crates/nexus-raw-core/src/nxr.rs"))
        .expect("nxr.rs reads");
    // Line endings differ by checkout (CRLF on a Windows runner), so the block
    // is delimited by whole lines, not by a byte pattern.
    let mut mutations: Vec<String> = Vec::new();
    let mut inside = false;
    for line in source.lines() {
        let line = line.trim_end();
        if line == "impl Nxr {" {
            inside = true;
            continue;
        }
        if inside && line == "}" {
            break;
        }
        if !inside {
            continue;
        }
        let line = line.trim_start();
        let rest = line
            .strip_prefix("pub async fn ")
            .or_else(|| line.strip_prefix("pub fn "));
        if let Some(rest) = rest {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            mutations.push(name);
        }
    }
    assert!(
        mutations.len() >= 30,
        "the parser found {} methods: it regressed",
        mutations.len()
    );
    let doc = std::fs::read_to_string(root.join("docs/reference/api.md")).expect("api.md reads");
    let missing: Vec<&String> = mutations
        .iter()
        .filter(|m| !doc.contains(&format!("`{m}(")))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/reference/api.md does not name these facade methods: {missing:?}"
    );
}

/// Every `Error` variant is named in the error reference: a new class cannot land
/// without the page that tells the user what it means and what to check next.
#[test]
fn the_error_reference_names_every_error_variant() {
    let root = repo_root();
    let source = std::fs::read_to_string(root.join("crates/nexus-raw-core/src/error.rs"))
        .expect("error.rs reads");
    let start = source.find("pub enum Error {").expect("the Error enum");
    let mut variants = Vec::new();
    let mut inside = false;
    for line in source[start..].lines() {
        let line = line.trim_end();
        if line == "pub enum Error {" {
            inside = true;
            continue;
        }
        if inside && line == "}" {
            break;
        }
        if !inside {
            continue;
        }
        let line = line.trim_start();
        if !line.starts_with(|c: char| c.is_ascii_uppercase()) {
            continue;
        }
        let name: String = line
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if !name.is_empty() {
            variants.push(name);
        }
    }
    assert!(
        variants.len() >= 15,
        "the parser found {} variants: it regressed",
        variants.len()
    );
    let doc =
        std::fs::read_to_string(root.join("docs/reference/errors.md")).expect("errors.md reads");
    let missing: Vec<&String> = variants
        .iter()
        .filter(|v| !doc.contains(&format!("`{v}")))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/reference/errors.md does not name these error variants: {missing:?}"
    );
}

/// `docs/reference/cli.md` carries a section per command and per `service` subcommand.
#[test]
fn the_cli_reference_covers_every_command() {
    let root = repo_root();
    let text = std::fs::read_to_string(root.join("docs/reference/cli.md")).expect("cli.md reads");
    let sections: BTreeSet<String> = text
        .lines()
        .filter_map(|l| l.strip_prefix("## nxr "))
        .map(|rest| {
            rest.split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let commands = commands_of(&help_of(&[]));
    // A command with subcommand sections (`channel get`, `service repos`) is covered.
    let covered = |c: &str| {
        sections
            .iter()
            .any(|s| s == c || s.starts_with(&format!("{c} ")))
    };
    let missing: Vec<&String> = commands.iter().filter(|c| !covered(c)).collect();
    assert!(
        missing.is_empty(),
        "docs/reference/cli.md lacks a section for: {missing:?}"
    );
    let stale: Vec<&String> = sections
        .iter()
        .filter(|s| {
            let first = s.split_once(' ').map_or(s.as_str(), |(head, _)| head);
            !commands.contains(first)
        })
        .collect();
    assert!(
        stale.is_empty(),
        "docs/reference/cli.md documents commands the binary does not carry: {stale:?}"
    );
    let subs = commands_of(&help_of(&["service".to_owned()]));
    let missing_subs: Vec<&String> = subs
        .iter()
        .filter(|s| !sections.contains(format!("service {s}").as_str()))
        .collect();
    assert!(
        missing_subs.is_empty(),
        "docs/reference/cli.md lacks a section for: {missing_subs:?}"
    );
}
