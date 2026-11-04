//! CLI conformance: `nxr doctor --deep` against the failure-scenario mocks.
//!
//! Covers: the all-ok hosted case, the range-stripped and search-dead warns (advice, exit 0), the absent status endpoint (the version probe silently absent, everything else lives), and the skipped verdict for an empty target.

use std::process::{Command, Output};

use mock_nexus::{MockNexus, Scenario};

const NXR: &str = env!("CARGO_BIN_EXE_nxr");

fn nxr(args: &[&str]) -> Output {
    Command::new(NXR)
        .args(args)
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD")
        .output()
        .expect("nxr runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn deep_args(srv: &MockNexus) -> [String; 4] {
    let url = format!("{}repository/raw-main/1.0.0/", srv.base_url());
    [
        "doctor".to_owned(),
        url,
        "--deep".to_owned(),
        "--json".to_owned(),
    ]
}

#[test]
fn deep_on_atomic_reports_all_probes() {
    let srv = MockNexus::start(Scenario::Atomic).expect("mock nexus starts");
    let a = deep_args(&srv);
    let out = nxr(&[a[0].as_str(), &a[1], &a[2], &a[3]]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "warns never fail: {}",
        stdout(&out)
    );
    let s = stdout(&out);
    assert!(
        s.contains(r#""check":"deep:version""#),
        "version probe: {s}"
    );
    assert!(s.contains(r#""check":"deep:search""#), "search probe: {s}");
    assert!(
        s.contains(r#""check":"deep:repo-type""#),
        "repo-type probe: {s}"
    );
    assert!(s.contains(r#""check":"deep:range""#), "range probe: {s}");
}

#[test]
fn deep_on_ignore_range_warns_range_stripped() {
    let srv = MockNexus::start(Scenario::IgnoreRange).expect("mock nexus starts");
    // The ignore-range store is atomic except the range behavior.
    // The target needs one object, so the test seeds it through the search-visible page
    // first: a put, which ignore-range stores normally.
    let dir = std::env::temp_dir().join(format!("nxr-doctor-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("probe.bin");
    std::fs::write(&file, b"probe bytes").expect("write fixture");
    let put = nxr(&[
        "put",
        &format!("{}repository/raw-main/1.0.0/probe.bin", srv.base_url()),
        "-f",
        file.to_str().expect("utf8"),
        "--sha",
    ]);
    assert_eq!(put.status.code(), Some(0), "put must succeed");
    let a = deep_args(&srv);
    let out = nxr(&[
        a[0].as_str(),
        &a[1],
        &a[2],
        &a[3],
        "--probe-name",
        "probe.bin",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a warn is advice: {}",
        stdout(&out)
    );
    let s = stdout(&out);
    assert!(
        s.contains("range-stripped"),
        "the 200-to-a-range warn must be named: {s}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A ghost repository on a search-400 server: the index cannot list it, which is
/// the search-dead warning, never a failed check.
#[test]
fn deep_on_a_ghost_repository_warns_search_dead() {
    let srv = MockNexus::start(Scenario::Search400).expect("mock nexus starts");
    let url = format!("{}repository/raw-ghost/1.0.0/", srv.base_url());
    let out = nxr(&["doctor", &url, "--deep"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a refused listing is advice: {}",
        stdout(&out)
    );
    let s = stdout(&out);
    assert!(
        s.contains("[ warn ] deep:search: warning: search-dead (400"),
        "the 400 warn is named: {s}"
    );
}

/// The search endpoint answers 404: the verdict is the absent-endpoint warning,
/// distinct from an index that answers an empty page.
#[test]
fn deep_on_an_absent_search_endpoint_warns_404() {
    let srv = MockNexus::start(Scenario::AssetsAbsent).expect("mock nexus starts");
    let a = deep_args(&srv);
    let out = nxr(&[a[0].as_str(), &a[1], &a[2], &a[3]]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let s = stdout(&out);
    assert!(
        s.contains("search-dead (404"),
        "the absent endpoint is named: {s}"
    );
    assert!(!s.contains("empty page"), "404 is not an empty page: {s}");
}

/// An index that answers with an empty page is alive: the probe says so.
#[test]
fn deep_on_an_empty_index_answers() {
    // No write yet: the lagging index serves the empty page.
    let srv = MockNexus::start(Scenario::SearchLag { lag_ms: 0 }).expect("mock nexus starts");
    let a = deep_args(&srv);
    let out = nxr(&[a[0].as_str(), &a[1], &a[2], &a[3]]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let s = stdout(&out);
    assert!(
        s.contains("the search index answers"),
        "an empty page is still an answer: {s}"
    );
}

/// A host with no service plane: the deep probes name the 404 of the missing
/// endpoint instead of calling it empty, count the warns, and still exit 0.
#[test]
fn deep_on_a_static_host_names_the_absent_service_plane() {
    let srv = MockNexus::start(Scenario::StaticHost).expect("mock nexus starts");
    let url = format!("{}repository/raw-main/1.0.0/", srv.base_url());
    let out = nxr(&["doctor", &url, "--deep"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let s = stdout(&out);
    for needle in [
        "deep:version: the status endpoint is absent (404",
        "deep:search: warning: search-dead (404",
        "deep:repo-type: skipped: the service plane is absent (404",
        "deep:eula: skipped: the endpoint is absent (404: no EULA gate on this server)",
        // The anonymous run warns about the missing credentials too.
        "all checks passed with 2 warning(s)",
    ] {
        assert!(s.contains(needle), "the probe reports {needle:?}: {s}");
    }
    assert!(
        s.contains("[ warn ] credentials:"),
        "the anonymous credentials check is a warn: {s}"
    );
    assert!(
        !s.contains("empty page"),
        "an absent endpoint is not an empty page: {s}"
    );
}

#[test]
fn deep_on_status_down_keeps_going() {
    let srv = MockNexus::start(Scenario::ServiceStatusDown).expect("mock nexus starts");
    let a = deep_args(&srv);
    let out = nxr(&[a[0].as_str(), &a[1], &a[2], &a[3]]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "one dead endpoint is diagnostic, not a failure: {}",
        stdout(&out)
    );
    let s = stdout(&out);
    assert!(
        s.contains("skipped") || s.contains("did not answer"),
        "the version probe reports the gap: {s}"
    );
}
/// The `--json` verdict class, per check: a script gates on `code` instead of
/// matching the prose in `detail`.
/// A stripped Range answers 200: the class says so, and the check still passes.
#[test]
fn codes_name_the_range_stripped_class() {
    let srv = MockNexus::start(Scenario::IgnoreRange).expect("mock nexus starts");
    let dir = std::env::temp_dir().join(format!("nxr-doctor-codes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("probe.bin");
    std::fs::write(&file, b"probe bytes").expect("write fixture");
    let put = nxr(&[
        "put",
        &format!("{}repository/raw-main/1.0.0/probe.bin", srv.base_url()),
        "-f",
        file.to_str().expect("utf8"),
        "--sha",
    ]);
    assert_eq!(put.status.code(), Some(0), "put must succeed");
    let a = deep_args(&srv);
    let out = nxr(&[
        a[0].as_str(),
        &a[1],
        &a[2],
        &a[3],
        "--probe-name",
        "probe.bin",
    ]);
    let row = code_of(&out, "deep:range");
    assert_eq!(row.1, "range-stripped", "the class names the strip");
    assert!(row.2, "a stripped Range is advice, not a failure");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Both dead search shapes carry the same class: an absent endpoint (404) and
/// an index that does not know the repository (400).
#[test]
fn codes_name_the_search_dead_class() {
    let srv = MockNexus::start(Scenario::Search400).expect("mock nexus starts");
    let url = format!("{}repository/raw-ghost/1.0.0/", srv.base_url());
    let out = nxr(&["doctor", &url, "--deep", "--json"]);
    assert_eq!(code_of(&out, "deep:search").1, "search-dead", "the 400");

    let srv = MockNexus::start(Scenario::AssetsAbsent).expect("mock nexus starts");
    let a = deep_args(&srv);
    let out = nxr(&[a[0].as_str(), &a[1], &a[2], &a[3]]);
    assert_eq!(code_of(&out, "deep:search").1, "search-dead", "the 404");
}

/// A check that passes says `ok`, and the anonymous credentials warning has its
/// own class instead of passing as one.
#[test]
fn codes_name_ok_and_anonymous() {
    let srv = MockNexus::start(Scenario::Atomic).expect("mock nexus starts");
    let a = deep_args(&srv);
    let out = nxr(&[a[0].as_str(), &a[1], &a[2], &a[3]]);
    let credentials = code_of(&out, "credentials");
    assert_eq!(credentials.1, "anonymous", "no creds is a named warning");
    assert!(credentials.2, "and it passes the check");
    // The atomic mock has no search endpoint: the dead search is the honest
    // class there, and a lagging index is where `ok` shows up.
    assert_eq!(code_of(&out, "deep:search").1, "search-dead", "no index");
    let lagging = MockNexus::start(Scenario::SearchLag { lag_ms: 0 }).expect("mock starts");
    let b = deep_args(&lagging);
    let out = nxr(&[b[0].as_str(), &b[1], &b[2], &b[3]]);
    let search = code_of(&out, "deep:search");
    assert_eq!(search.1, "ok", "a live index is ok");
    let range = code_of(&out, "deep:range");
    assert!(
        range.1 == "ok" || range.1 == "skipped",
        "an empty store skips the range probe, never mislabels it: {}",
        range.1
    );
}

/// A rejected probe names auth, not a generic failure: the exit code is 3, and
/// the class tells the operator to fix the credentials.
#[test]
fn codes_name_the_auth_rejected_class() {
    let srv = MockNexus::start(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    })
    .expect("mock nexus starts");
    let url = format!("{}repository/raw-main/1.0.0/", srv.base_url());
    let out = nxr(&["doctor", &url, "--json", "-u", "someone:wrong"]);
    let probe = code_of(&out, "probe");
    assert_eq!(probe.1, "auth-rejected", "the class names the 401");
    assert!(!probe.2, "a rejected probe fails the check");
}

/// A server that does not answer fails the probe, and that is the plain
/// `failed` class: not a skip, not a warning.
#[test]
fn codes_name_the_failed_class() {
    let out = nxr(&[
        "doctor",
        "http://127.0.0.1:1/repository/raw-main/",
        "--json",
    ]);
    let probe = code_of(&out, "probe");
    assert_eq!(probe.1, "failed", "an unreachable server fails the probe");
}

/// `--tls-insecure` has its own class, so a CI job can tell it apart from a
/// server problem.
#[test]
fn codes_name_the_insecure_class() {
    let srv = MockNexus::start(Scenario::Atomic).expect("mock nexus starts");
    let a = deep_args(&srv);
    let out = nxr(&[a[0].as_str(), &a[1], &a[2], &a[3], "--tls-insecure"]);
    let tls = code_of(&out, "tls");
    assert_eq!(tls.1, "insecure", "the flag has its own class");
    assert!(!tls.2, "and it still fails the check");
}

/// A probe with nothing to judge says `skipped`, an answered 404 says `absent`:
/// neither passes as `ok`.
#[test]
fn codes_name_skipped_and_absent() {
    let srv = MockNexus::start(Scenario::StaticHost).expect("mock nexus starts");
    let url = format!("{}repository/raw-main/1.0.0/", srv.base_url());
    let out = nxr(&["doctor", &url, "--deep", "--json"]);
    assert_eq!(code_of(&out, "deep:version").1, "absent");
    assert_eq!(code_of(&out, "deep:repo-type").1, "skipped");
    assert_eq!(code_of(&out, "deep:eula").1, "skipped");
}

/// Parse one `--json` report row into `(check, code, ok)`, failing loudly when
/// the check is missing.
fn code_of(out: &Output, name: &str) -> (String, String, bool) {
    let text = stdout(out);
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .find_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).expect("a json row");
            (v["check"].as_str() == Some(name)).then(|| {
                (
                    v["check"].as_str().unwrap_or_default().to_owned(),
                    v["code"].as_str().unwrap_or_default().to_owned(),
                    v["ok"].as_bool().unwrap_or(false),
                )
            })
        })
        .unwrap_or_else(|| panic!("no {name} row in: {text}"))
}
