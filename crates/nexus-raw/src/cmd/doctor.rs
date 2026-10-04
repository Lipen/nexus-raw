//! `nxr doctor`: credentials, TLS and reachability, without printing secrets (§5.4).

use nexus_raw_core::{config::normalize_base, creds, Error};

use crate::Cli;

pub(crate) async fn run(cli: &Cli, url: Option<&str>) -> Result<(), Error> {
    let mut report: Vec<(&'static str, bool, String)> = Vec::new();
    // Warns pass the check (no exit-code impact) but deserve their own marker in both renders.
    let mut warns: std::collections::HashSet<&'static str> = std::collections::HashSet::new();

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
            if has {
                report.push(("credentials", true, format!("resolved from {source}")));
            } else {
                // Anonymous access is a legitimate configuration: the gap is a warning, not a failure.
                warns.insert("credentials");
                report.push((
                    "credentials",
                    true,
                    "warning: none found; anonymous requests go out unauthenticated (pass -u or export NXR_AUTH)".to_owned(),
                ));
            }
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
        cli.workers > 0
            && cli.workers <= 64
            && cli.retry >= 1
            && cli.connect_timeout_secs >= 1
            && cli.stall_secs >= 1,
        format!(
            "workers {}, retry {}, stall {}s, connect {}s",
            cli.workers, cli.retry, cli.stall_secs, cli.connect_timeout_secs
        ),
    ));

    // Proxy awareness: the names only, never the values (they can carry credentials).
    let proxies: Vec<&str> = [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ]
    .iter()
    .copied()
    .filter(|v| std::env::var_os(v).is_some_and(|x| !x.is_empty()))
    .collect();
    report.push((
        "proxy",
        true,
        if proxies.is_empty() {
            "no proxy in the environment".to_owned()
        } else {
            format!("environment proxy: {}", proxies.join(", "))
        },
    ));

    let mut plaintext_host: Option<String> = None;
    if let Some(url) = url {
        match normalize_base(url) {
            Ok(base) => {
                // Credentials over plaintext http to anything but localhost is a
                // configuration worth naming, whatever the probe answers.
                if auth.is_some() && base.starts_with("http://") {
                    let host = base
                        .trim_start_matches("http://")
                        .split(['/', ':'])
                        .next()
                        .unwrap_or("")
                        .to_owned();
                    if host != "localhost" && host != "127.0.0.1" && host != "[::1]" {
                        plaintext_host = Some(host);
                    }
                }
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
                            // A 401 or 403 means the server rejected the call: the probe is a
                            // credentials failure, not a pass, whatever the status table says.
                            let rejected = info.status == 401 || info.status == 403;
                            let ok = !rejected && info.status < 500;
                            let detail = if rejected {
                                format!("HEAD {url} → HTTP {}: credentials rejected", info.status)
                            } else {
                                format!("HEAD {url} → HTTP {}", info.status)
                            };
                            report.push(("probe", ok, detail));
                        }
                        Err(e) => report.push(("probe", false, e.to_string())),
                    },
                    Err(e) => report.push(("probe", false, e.to_string())),
                }
            }
            Err(e) => report.push(("url", false, e.to_string())),
        }
    }
    if let Some(host) = plaintext_host {
        warns.insert("plaintext");
        report.push((
            "plaintext",
            true,
            format!("warning: Basic credentials travel over plaintext http to {host}"),
        ));
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
            let mut line = serde_json::json!({"check": name, "ok": ok, "detail": detail});
            if warns.contains(name) {
                line["warn"] = serde_json::Value::Bool(true);
            }
            println!("{line}");
        }
    } else {
        println!("doctor:");
        for (name, ok, detail) in &report {
            let mark = if !ok {
                " FAIL "
            } else if warns.contains(name) {
                " warn "
            } else {
                "  ok  "
            };
            if !ok {
                failures += 1;
                if matches!(*name, "probe") {
                    transport_failures += 1;
                }
            }
            println!("  [{mark}] {name}: {detail}");
        }
        if failures == 0 && warns.is_empty() {
            println!("  all checks passed");
        } else if failures == 0 {
            println!("  all checks passed with {} warning(s)", warns.len());
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
