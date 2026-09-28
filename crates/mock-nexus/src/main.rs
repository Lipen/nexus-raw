//! `mock-nexus` binary: serve one failure scenario forever.
//!
//! ```text
//! mock-nexus <scenario> [--port N] [--chunk-delay-ms N] [--chunk-size N]
//!                       [--partial-bytes N] [--flaky K] [--auth user:pass]
//! ```

use mock_nexus::{MockNexus, Scenario};
use std::io::Write;
use std::net::SocketAddr;

/// Flag values, with the documented defaults.
#[derive(Debug)]
struct Flags {
    port: u16,
    chunk_delay_ms: u64,
    chunk_size: usize,
    partial_bytes: usize,
    flaky: u32,
    user: String,
    pass: String,
}

impl Default for Flags {
    fn default() -> Self {
        Self {
            port: 0,
            chunk_delay_ms: 90,
            chunk_size: 1024,
            partial_bytes: 64,
            flaky: 2,
            user: "ci".to_owned(),
            pass: "secret".to_owned(),
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Machine-readable scenario table for external conformance suites (§5c):
    // `mock-nexus --print-scenarios` prints the names, one JSON array.
    if argv.as_slice() == ["--print-scenarios"] {
        print_scenarios(std::io::stdout().lock()).expect("stdout write");
        return;
    }
    let Some(name) = argv.first() else {
        fail_usage();
    };
    if name.starts_with('-') {
        fail_usage();
    }
    let mut flags = Flags::default();
    if let Err(msg) = parse_flags(&argv[1..], &mut flags) {
        eprintln!("mock-nexus: {msg}");
        fail_usage();
    }
    let Some(scenario) = build_scenario(name, &flags) else {
        eprintln!("mock-nexus: unknown scenario '{name}'");
        fail_usage();
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], flags.port));
    let server = match MockNexus::start_on(scenario, addr) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("mock-nexus: cannot bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    println!("listening http://127.0.0.1:{}", server.addr().port());
    let _ = std::io::stdout().flush();

    // Keep the handle alive (dropping it would stop the server) and serve
    // forever.
    // SIGTERM/SIGINT kill us.
    loop {
        std::thread::park();
    }
}

/// Map a scenario name from [`mock_nexus::SCENARIOS`] to its variant.
fn build_scenario(name: &str, flags: &Flags) -> Option<Scenario> {
    match name {
        "atomic" => Some(Scenario::Atomic),
        "partial-put" => Some(Scenario::PartialPut {
            first_attempt_bytes: flags.partial_bytes,
        }),
        "drop-connection" => Some(Scenario::DropConnection),
        "freeze-upload" => Some(Scenario::FreezeUpload),
        "slow" => Some(Scenario::Slow {
            chunk_delay_ms: flags.chunk_delay_ms,
            chunk_size: flags.chunk_size,
        }),
        "foreign-marker" => Some(Scenario::ForeignMarker),
        "markerless" => Some(Scenario::Markerless),
        "auth-401" => Some(Scenario::Auth401 {
            user: flags.user.clone(),
            pass: flags.pass.clone(),
        }),
        "doc-drift" => Some(Scenario::DocDrift),
        "flaky" => Some(Scenario::Flaky {
            first_failures: flags.flaky,
        }),
        _ => None,
    }
}

/// Parse the flag list into `flags`.
/// Values come as `--flag value` pairs.
fn parse_flags(args: &[String], flags: &mut Flags) -> Result<(), String> {
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag {
            "--port" => flags.port = parse_num(flag, value)?,
            "--chunk-delay-ms" => flags.chunk_delay_ms = parse_num(flag, value)?,
            "--chunk-size" => flags.chunk_size = parse_num(flag, value)?,
            "--partial-bytes" => flags.partial_bytes = parse_num(flag, value)?,
            "--flaky" => flags.flaky = parse_num(flag, value)?,
            "--auth" => {
                let Some((user, pass)) = value.split_once(':') else {
                    return Err(format!("--auth expects user:pass, got '{value}'"));
                };
                flags.user = user.to_owned();
                flags.pass = pass.to_owned();
            }
            other => return Err(format!("unknown flag '{other}'")),
        }
        i += 2;
    }
    Ok(())
}

fn parse_num<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse::<T>()
        .map_err(|_| format!("invalid value for {flag}: '{value}'"))
}

/// Print the shared scenario table as JSON for external conformance suites.
fn print_scenarios<W: std::io::Write>(mut out: W) -> std::io::Result<()> {
    writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(mock_nexus::SCENARIOS)
            .expect("scenario names are a static string table")
    )
}

fn fail_usage() -> ! {
    eprintln!(
        "usage: mock-nexus <scenario> [--port N] [--chunk-delay-ms N] [--chunk-size N] \
         [--partial-bytes N] [--flaky K] [--auth user:pass]"
    );
    eprintln!("       mock-nexus --print-scenarios");
    eprintln!("scenarios: {}", mock_nexus::SCENARIOS.join(", "));
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::{build_scenario, parse_flags, print_scenarios, Flags};
    use mock_nexus::Scenario;

    #[test]
    fn print_scenarios_lists_the_shared_table() {
        let mut buf = Vec::new();
        print_scenarios(&mut buf).unwrap();
        let parsed: Vec<String> = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed, mock_nexus::SCENARIOS);
        assert!(parsed.contains(&"atomic".to_owned()));
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_all_flags() {
        let mut flags = Flags::default();
        parse_flags(
            &args(&[
                "--port",
                "8080",
                "--chunk-delay-ms",
                "25",
                "--chunk-size",
                "512",
                "--partial-bytes",
                "128",
                "--flaky",
                "5",
                "--auth",
                "alice:wonder",
            ]),
            &mut flags,
        )
        .unwrap();
        assert_eq!(flags.port, 8080);
        assert_eq!(flags.chunk_delay_ms, 25);
        assert_eq!(flags.chunk_size, 512);
        assert_eq!(flags.partial_bytes, 128);
        assert_eq!(flags.flaky, 5);
        assert_eq!(flags.user, "alice");
        assert_eq!(flags.pass, "wonder");
    }

    #[test]
    fn rejects_bad_flags_and_values() {
        let mut flags = Flags::default();
        assert!(parse_flags(&args(&["--nope", "1"]), &mut flags).is_err());
        assert!(parse_flags(&args(&["--port"]), &mut flags).is_err());
        assert!(parse_flags(&args(&["--port", "http"]), &mut flags).is_err());
        assert!(parse_flags(&args(&["--auth", "no-colon"]), &mut flags).is_err());
    }

    #[test]
    fn builds_every_scenario_from_defaults() {
        let flags = Flags::default();
        for name in mock_nexus::SCENARIOS {
            assert!(build_scenario(name, &flags).is_some(), "{name} missing");
        }
        assert!(build_scenario("bogus", &flags).is_none());
    }

    #[test]
    fn scenarios_pick_up_flag_values() {
        let mut flags = Flags::default();
        parse_flags(
            &args(&[
                "--partial-bytes",
                "9",
                "--flaky",
                "3",
                "--chunk-size",
                "7",
                "--chunk-delay-ms",
                "11",
                "--auth",
                "u:p",
            ]),
            &mut flags,
        )
        .unwrap();
        assert_eq!(
            build_scenario("partial-put", &flags),
            Some(Scenario::PartialPut {
                first_attempt_bytes: 9
            })
        );
        assert_eq!(
            build_scenario("flaky", &flags),
            Some(Scenario::Flaky { first_failures: 3 })
        );
        assert_eq!(
            build_scenario("slow", &flags),
            Some(Scenario::Slow {
                chunk_delay_ms: 11,
                chunk_size: 7
            })
        );
        assert_eq!(
            build_scenario("auth-401", &flags),
            Some(Scenario::Auth401 {
                user: "u".to_owned(),
                pass: "p".to_owned()
            })
        );
    }
}
