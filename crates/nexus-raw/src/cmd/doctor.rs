//! `nxr doctor`: credentials, TLS and reachability, without printing secrets (§5.4).

use crate::render::outln;
use nexus_raw_core::{config::normalize_base, creds, Error};

use crate::Cli;

pub(crate) async fn run(
    cli: &Cli,
    url: Option<&str>,
    deep: bool,
    probe_name: Option<&str>,
) -> Result<(), Error> {
    let mut report: Vec<(&'static str, bool, String, Option<&'static str>)> = Vec::new();
    // Warns pass the check (no exit-code impact) but deserve their own marker in both renders.
    let mut warns: std::collections::HashSet<&'static str> = std::collections::HashSet::new();

    // Credentials: resolved presence only, never values.
    let explicit = cli.user.as_deref();
    // The alias pair wins over ambient env but never over -u (§ priority).
    let alias_pair = if explicit.is_none() {
        cli.alias_creds
            .as_ref()
            .map(|(u, p)| (u.as_str(), p.as_str()))
    } else {
        None
    };
    let resolved = match alias_pair {
        Some((u, p)) => creds::resolve(Some((u, p)))?.map(|c| c.header),
        None => split_and_resolve(explicit)?,
    };
    let auth = {
        let source = if explicit.is_some() {
            "-u flag"
        } else if alias_pair.is_some() {
            // The pair came from the alias: name it, never claim -u.
            "the -R alias"
        } else if std::env::var_os("NXR_AUTH").is_some_and(|v| !v.is_empty()) {
            "NXR_AUTH"
        } else {
            "NXR_USERNAME + NXR_PASSWORD"
        };
        let has = resolved.is_some();
        if has {
            report.push(("credentials", true, format!("resolved from {source}"), None));
        } else {
            // Anonymous access is a legitimate configuration: the gap is a warning, not a failure.
            warns.insert("credentials");
            report.push((
                "credentials",
                true,
                "warning: none found; anonymous requests go out unauthenticated (pass -u or export NXR_AUTH)".to_owned(),
                None,
            ));
        }
        resolved
    };

    // --tls-insecure counts as a failed check: the report flags it, it does not hide it.
    if cli.tls_insecure {
        report.push((
            "tls",
            false,
            "verification is OFF (--tls-insecure); fine for a local mock, dangerous beyond it"
                .to_owned(),
            Some("drop --tls-insecure, or install a real certificate on the server"),
        ));
    } else {
        report.push((
            "tls",
            true,
            if cli.ca_bundle.is_some() {
                "verification is ON with a custom CA bundle".to_owned()
            } else {
                "verification is ON".to_owned()
            },
            None,
        ));
        if let Some(path) = &cli.ca_bundle {
            report.push(("ca", true, format!("bundle: {}", path.display()), None));
        }
    }

    report.push((
        "settings",
        cli.workers > 0
            && cli.workers <= 64
            && cli.retry >= 1
            && cli.connect_timeout_secs >= 1
            && cli.stall_secs >= 1,
        format!(
            "workers {}, retry {} (probes run once), stall {}s, connect {}s",
            cli.workers, cli.retry, cli.stall_secs, cli.connect_timeout_secs
        ),
        None,
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
        None,
    ));

    let mut plaintext_host: Option<String> = None;
    // The probe rejected the credentials (401/403): the aggregate error then
    // carries the auth hint, not the generic network one.
    let mut probe_auth_rejected = false;
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
                // The probe asks the service status endpoint first (computed
                // before `base` moves into the config): a repository-root HEAD
                // can answer 400 on URLs that are not a plain object path,
                // which read as a failure. Any non-5xx answer still proves
                // reachability, and the line says so, because a bare `400`
                // reads as a bug.
                let status_url = format!("{}/service/rest/v1/status", server_origin(&base)?);
                let cfg = nexus_raw_core::Config {
                    base: base.clone(),
                    tls_insecure: cli.tls_insecure,
                    ca_bundle: cli.ca_bundle.clone(),
                    workers: 1,
                    retry_attempts: 1,
                    connect_timeout: std::time::Duration::from_secs(cli.connect_timeout_secs),
                    stall_timeout: std::time::Duration::from_secs(cli.stall_secs),
                    auth,
                };
                match nexus_raw_core::NexusClient::new(&cfg, dead_progress()) {
                    Ok(client) => {
                        let mut next: Option<&'static str> = None;
                        let detail;
                        let ok;
                        match client.get_small(&status_url).await {
                            Ok(Some(_)) => {
                                ok = true;
                                detail =
                                    "status endpoint answered: the server is reachable".to_owned();
                            }
                            // `Ok(None)` is a 404: the endpoint is absent, not
                            // a pass. Fall through to the repository HEAD, which
                            // is what the credentials probe is for.
                            _ => match client.head_info(url).await {
                                Ok(info) => {
                                    // A 401 or 403 means the server rejected the
                                    // call: a credentials failure, not a pass.
                                    let rejected = info.status == 401 || info.status == 403;
                                    ok = !rejected && info.status < 500;
                                    if rejected {
                                        probe_auth_rejected = true;
                                        next = Some(
                                            "pass -u user:pass, or export NXR_AUTH (base64 user:pass), or NXR_USERNAME + NXR_PASSWORD",
                                        );
                                    } else if info.status >= 500 {
                                        next = Some(
                                            "the server is up but unhealthy: check the Nexus logs, retry when it answers 2xx or 4xx",
                                        );
                                    }
                                    detail = format!(
                                        "HEAD {url} → HTTP {}: any non-5xx means the server is reachable",
                                        info.status
                                    );
                                }
                                Err(e) => {
                                    ok = false;
                                    next = Some(
                                        "check the host and port, TLS trust, and that the URL is the Nexus base (…/nexus): nxr doctor <url> --deep names the failure",
                                    );
                                    detail = e.to_string();
                                }
                            },
                        }
                        report.push(("probe", ok, detail, next));
                        if deep {
                            match deep_probes(cli, &client, &base, probe_name).await {
                                Ok(lines) => {
                                    for (name, ok, detail, next) in lines {
                                        // A deep verdict that warns gets the same
                                        // marker and count as every other warning.
                                        if detail.starts_with("warning: ") {
                                            warns.insert(name);
                                        }
                                        report.push((name, ok, detail, next));
                                    }
                                }
                                Err(e) => report.push(("deep", false, e.to_string(), None)),
                            }
                        }
                    }
                    Err(e) => report.push(("probe", false, e.to_string(), None)),
                }
            }
            Err(e) => report.push(("url", false, e.to_string(), None)),
        }
    }
    if let Some(host) = plaintext_host {
        warns.insert("plaintext");
        report.push((
            "plaintext",
            true,
            format!("warning: Basic credentials travel over plaintext http to {host}"),
            None,
        ));
    }

    let mut failures = 0usize;
    let mut transport_failures = 0usize;
    if cli.json {
        for (name, ok, detail, next) in &report {
            if !*ok {
                failures += 1;
                if matches!(*name, "probe") {
                    transport_failures += 1;
                }
            }
            let mut line = serde_json::json!({"check": name, "ok": ok, "detail": detail});
            if let Some(n) = next {
                line["next"] = serde_json::Value::String((*n).to_owned());
            }
            if warns.contains(name) {
                line["warn"] = serde_json::Value::Bool(true);
            }
            outln!("{line}");
        }
    } else {
        outln!("doctor:");
        for (name, ok, detail, next) in &report {
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
            outln!("  [{mark}] {name}: {detail}");
            if let Some(n) = next {
                outln!("        next: {n}");
            }
        }
        if failures == 0 && warns.is_empty() {
            outln!("  all checks passed");
        } else if failures == 0 {
            outln!("  all checks passed with {} warning(s)", warns.len());
        }
    }
    if failures == 0 {
        Ok(())
    } else if probe_auth_rejected {
        // The only failing probe was a 401/403: credentials are the problem,
        // so the aggregate carries the auth hint, not the network one.
        Err(Error::Auth {
            url: url.unwrap_or("-").to_owned(),
            reason: format!("{failures} check(s) failed"),
            detail: None,
        })
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

/// One deep-probe verdict: the check name, whether it passed, the detail,
/// and the remediation a failed check suggests (`None` for passes and skips).
/// A `false` that names a warn (through the report's warn set upstream) is
/// advice, never a failure.
type DeepLine = (&'static str, bool, String, Option<&'static str>);

/// The read-only `--deep` probes of the server's actual behavior.
///
/// Every probe is independent: one refusing never stops the others.
/// A probe with a target (range, content-length) needs an object.
/// Without one the verdict is `skipped`, never `ok` and never `fail` (an empty repository
/// is a normal thing to point doctor at, before the first `up`).
/// Write rights are deliberately not probed: a PUT probe would itself be a
/// mutation, against the read-only contract of the diagnostics.
async fn deep_probes(
    cli: &Cli,
    client: &nexus_raw_core::NexusClient,
    base: &str,
    probe_name: Option<&str>,
) -> Result<Vec<DeepLine>, Error> {
    let _ = cli;
    let mut out: Vec<DeepLine> = Vec::new();
    let root = base.trim_end_matches('/').to_owned();
    let origin = server_origin(base)?;
    let status_url = format!("{origin}/service/rest/v1/status");

    // version: diagnostic only, never a gate.
    match client.get_small(&status_url).await {
        Ok(Some(body)) => {
            let text = String::from_utf8_lossy(&body);
            let version = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v["version"].as_str().map(str::to_owned));
            out.push((
                "deep:version",
                true,
                version.unwrap_or_else(|| "status endpoint answered".to_owned()),
                None,
            ));
        }
        Ok(None) => out.push((
            "deep:version",
            true,
            "the status endpoint is absent (404: the service plane is missing)".to_owned(),
            None,
        )),
        Err(_) => out.push((
            "deep:version",
            true,
            "skipped: the status endpoint did not answer (diagnostic only)".to_owned(),
            None,
        )),
    }

    // range + content-length on the resolved target object.
    match resolve_probe_target(client, &origin, &root, probe_name).await {
        Some(object_url) => match client.head_info(&object_url).await {
            Ok(info) => {
                let len = info.size;
                match len {
                    Some(n) => {
                        out.push(("deep:content-length", true, format!("present ({n})"), None))
                    }
                    None => out.push((
                        "deep:content-length",
                        true,
                        "warning: absent (resume and --segments are limited)".to_owned(),
                        None,
                    )),
                }
                match client.probe_range(&object_url).await {
                    Ok(206) => out.push(("deep:range", true, "206 honored".to_owned(), None)),
                    Ok(200) => out.push((
                        "deep:range",
                        true,
                        "warning: range-stripped (200 to a ranged GET: --segments degrades to the single stream)".to_owned(),
                        None,
                    )),
                    Ok(code) => out.push((
                        "deep:range",
                        false,
                        format!("ranged GET answered HTTP {code}"),
                        Some("the server or a proxy strips Range: expect full-file downloads, --segments will not resume"),
                    )),
                    Err(e) => out.push(("deep:range", false, e.to_string(), None)),
                }
            }
            Err(e) => {
                out.push((
                    "deep:content-length",
                    true,
                    format!("skipped: HEAD failed ({e})"),
                    None,
                ));
                out.push(("deep:range", true, "skipped: HEAD failed".to_owned(), None));
            }
        },
        None => {
            out.push((
                "deep:content-length",
                true,
                "skipped: no object to probe (an empty repository is a normal case)".to_owned(),
                None,
            ));
            out.push((
                "deep:range",
                true,
                "skipped: no object to probe (an empty repository is a normal case)".to_owned(),
                None,
            ));
        }
    }

    // search liveness: one page against the first repository path segment.
    let repo = base
        .split("/repository/")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("raw-main")
        .to_owned();
    let search_url = format!("{origin}/service/rest/v1/search/assets?repository={repo}");
    match client.get_small(&search_url).await {
        Ok(Some(_)) => out.push(("deep:search", true, "the search index answers".to_owned(), None)),
        // `get_small` maps a 404 to `None`: the endpoint is absent, not empty.
        Ok(None) => out.push((
            "deep:search",
            true,
            "warning: search-dead (404: the search endpoint is absent; diff --via search refuses there, --fallback saves it)".to_owned(),
            None,
        )),
        // A ghost repository: the index answers 400, and listing cannot work there.
        // `get_small` maps a 400 to the generic HTTP error, so the class is the
        // status, not `SearchRepoMissing` (which only the walk layer raises).
        Err(nexus_raw_core::Error::Http { status: 400, .. }) => out.push((
            "deep:search",
            true,
            "warning: search-dead (400: the repository is unknown to the index; diff --via search refuses there, --fallback saves it)".to_owned(),
            None,
        )),
        Err(e) => out.push(("deep:search", false, e.to_string(), None)),
    }

    // repo type through the service REST API, read-only.
    let repos_url = format!("{origin}/service/rest/v1/repositories");
    match client.get_small(&repos_url).await {
        Ok(Some(body)) => {
            let parsed: Option<serde_json::Value> = serde_json::from_slice(&body).ok();
            let entry = parsed.as_ref().and_then(|v| v.as_array()).and_then(|a| {
                a.iter().find(|r| {
                    r["name"]
                        .as_str()
                        .is_some_and(|n| base.contains(&format!("/repository/{n}/")))
                })
            });
            match entry {
                Some(r) => {
                    let kind = r["type"].as_str().unwrap_or("?");
                    let detail = if kind == "group" {
                        format!(
                            "group: {}",
                            r["members"]
                                .as_array()
                                .map(|m| m
                                    .iter()
                                    .filter_map(|x| x.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", "))
                                .unwrap_or_else(|| "members not reported".to_owned())
                        )
                    } else {
                        format!("{kind}, format {}", r["format"].as_str().unwrap_or("?"))
                    };
                    out.push(("deep:repo-type", true, detail, None));
                }
                None => out.push((
                    "deep:repo-type",
                    true,
                    "skipped: the repository is not in the service listing".to_owned(),
                    None,
                )),
            }
        }
        Ok(None) => out.push((
            "deep:repo-type",
            true,
            "skipped: the service plane is absent (404: the repositories document lives under /service/rest/v1/repositories)".to_owned(),
            None,
        )),
        Err(_) => out.push((
            "deep:repo-type",
            true,
            "skipped: the service REST endpoint did not answer".to_owned(),
            None,
        )),
    }

    // eula gate: the CE 3.79+ write gate, strictly read-only (a GET).
    // A 404 means the server predates the gate.
    // A 401/403 means the probe needs admin rights, which is a skip, not a finding.
    let eula_url = format!("{origin}/service/rest/v1/system/eula");
    match client.get_small(&eula_url).await {
        Ok(Some(body)) => {
            let parsed: Option<serde_json::Value> = serde_json::from_slice(&body).ok();
            match parsed.as_ref().and_then(|v| v["accepted"].as_bool()) {
                Some(true) => {
                    out.push(("deep:eula", true, "accepted (writes are not gated)".to_owned(), None))
                }
                Some(false) => out.push((
                    "deep:eula",
                    true,
                    "warning: not accepted (writes answer 403 with the EULA body until POST /service/rest/v1/system/eula echoes the disclaimer back)".to_owned(),
                    Some("nxr service eula <url> --accept (needs nx-admin): until then every PUT answers 403"),
                )),
                None => out.push((
                    "deep:eula",
                    true,
                    "skipped: the endpoint answered without an accepted field".to_owned(),
                    None,
                )),
            }
        }
        Ok(None) => out.push((
            "deep:eula",
            true,
            "skipped: the endpoint is absent (404: no EULA gate on this server)".to_owned(),
            None,
        )),
        Err(_) => out.push((
            "deep:eula",
            true,
            "skipped: the endpoint refused the probe (admin rights are required; the refusal is not a finding)".to_owned(),
            None,
        )),
    }

    Ok(out)
}

/// The server origin of a base URL: scheme, host and port, with no repository path.
fn server_origin(base: &str) -> Result<String, Error> {
    let (scheme, rest) = if let Some(rest) = base.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = base.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(Error::Misuse(format!("base URL: {base:?} is not http(s)")));
    };
    let host_port = rest.split('/').next().unwrap_or_default();
    if host_port.is_empty() {
        return Err(Error::Misuse(format!("base URL: {base:?} has no host")));
    }
    Ok(format!("{scheme}://{host_port}"))
}

/// Resolve the object the range and content-length probes target: the explicit
/// name, else the first object one search page returns.
/// `None` means no target exists, which is a skip, not a failure.
async fn resolve_probe_target(
    client: &nexus_raw_core::NexusClient,
    origin: &str,
    root: &str,
    probe_name: Option<&str>,
) -> Option<String> {
    if let Some(n) = probe_name {
        // The name rides the base as the user typed it: the base already
        // points at the directory the object lives under.
        return Some(format!("{root}/{n}"));
    }
    let repo = root
        .split("/repository/")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("raw-main");
    let search_url = format!("{origin}/service/rest/v1/search/assets?repository={repo}");
    let body = client.get_small(&search_url).await.ok()??;
    let parsed: serde_json::Value = serde_json::from_slice(&body).ok()?;
    let first = parsed["items"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|i| i["path"].as_str())
        .map(|p| p.trim_start_matches('/').to_owned())?;
    Some(format!("{origin}/repository/{repo}/{first}"))
}
