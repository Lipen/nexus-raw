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
