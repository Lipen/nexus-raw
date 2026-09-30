//! `nxr doctor`: credentials, TLS and reachability, without printing secrets (§5.4).

use nexus_raw_core::{config::normalize_base, creds, Error};

use crate::Cli;

pub(crate) async fn run(cli: &Cli, url: Option<&str>) -> Result<(), Error> {
    let mut report: Vec<(&'static str, bool, String)> = Vec::new();

    // Credentials: resolved presence only, never values.
    let explicit = cli.user.as_deref();
    let auth = match split_and_resolve(explicit) {
        Ok(header) => {
            let source = if explicit.is_some() {
                "-u flag"
            } else if std::env::var_os("NXR_AUTH").is_some_and(|v| !v.is_empty()) {
                "NXR_AUTH"
            } else {
                "NXR_USERNAME + NXR_PASSWORD"
            };
            let has = header.is_some();
            report.push((
                "credentials",
                has,
                if has {
                    format!("resolved from {source}")
                } else {
                    "none found: anonymous requests; pass -u or export NXR_AUTH".to_owned()
                },
            ));
            header
        }
        Err(e) => {
            report.push(("credentials", false, e.to_string()));
            None
        }
    };

    // --tls-insecure counts as a failed check: the report flags it, it does not hide it.
    if cli.tls_insecure {
        report.push((
            "tls",
            false,
            "verification is OFF (--tls-insecure); fine for a local mock, dangerous beyond it"
                .to_owned(),
        ));
    } else {
        report.push(("tls", true, "verification is ON".to_owned()));
    }

    report.push((
        "settings",
        cli.workers > 0 && cli.workers <= 64,
        format!(
            "workers {}, retry {}, stall {}s, connect {}s",
            cli.workers, cli.retry, cli.stall_secs, cli.connect_timeout_secs
        ),
    ));

    if let Some(url) = url {
        match normalize_base(url) {
            Ok(base) => {
                let cfg = nexus_raw_core::Config {
                    base,
                    tls_insecure: cli.tls_insecure,
                    workers: 1,
                    retry_attempts: 1,
                    connect_timeout: std::time::Duration::from_secs(cli.connect_timeout_secs),
                    stall_timeout: std::time::Duration::from_secs(cli.stall_secs),
                    auth,
                };
                match nexus_raw_core::NexusClient::new(&cfg, dead_progress()) {
                    Ok(client) => match client.head_info(url).await {
                        Ok(info) => {
                            let ok = info.status < 500;
                            report.push((
                                "probe",
                                ok,
                                format!("HEAD {url} → HTTP {}", info.status),
                            ));
                        }
                        Err(e) => report.push(("probe", false, e.to_string())),
                    },
                    Err(e) => report.push(("probe", false, e.to_string())),
                }
            }
            Err(e) => report.push(("url", false, e.to_string())),
        }
    }

    let mut failures = 0usize;
    let mut transport_failures = 0usize;
    if cli.json {
        for (name, ok, detail) in &report {
            if !*ok {
                failures += 1;
                if matches!(*name, "probe") {
                    transport_failures += 1;
                }
            }
            println!(
                "{}",
                serde_json::json!({"check": name, "ok": ok, "detail": detail})
            );
        }
    } else {
        println!("doctor:");
        for (name, ok, detail) in &report {
            let mark = if *ok { "  ok  " } else { " FAIL " };
            if !ok {
                failures += 1;
                if matches!(*name, "probe") {
                    transport_failures += 1;
                }
            }
            println!("  [{mark}] {name}: {detail}");
        }
        if failures == 0 {
            println!("  all checks passed");
        }
    }
    if failures == 0 {
        Ok(())
    } else if transport_failures > 0 {
        Err(Error::Transport {
            url: url.unwrap_or("-").to_owned(),
            detail: format!("{failures} check(s) failed"),
        })
    } else {
        Err(Error::Misuse(format!("{failures} check(s) failed")))
    }
}

fn split_and_resolve(explicit: Option<&str>) -> Result<Option<String>, Error> {
    let pair = match explicit {
        Some(u) => match u.split_once(':') {
            Some((user, pass)) => Some((user, pass)),
            None => {
                return Err(Error::Misuse(
                    "-u expects user:pass (a single ':'); the value carries none".to_owned(),
                ))
            }
        },
        None => None,
    };
    Ok(creds::resolve(pair)?.map(|c| c.header))
}

/// A progress channel into nowhere: doctor renders its own report.
fn dead_progress() -> nexus_raw_core::Progress {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    nexus_raw_core::Progress::new(tx)
}
