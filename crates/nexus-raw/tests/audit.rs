//! CLI conformance: `nxr audit` against the `audit-litter` scenario.
//!
//! Covers: the structural classes (orphan, dangling marker, unparseable marker), the clean pair never flagged, the exit-code verdict, and the two-mode contract (the structural pass sees no byte corruption; the byte pass verifies the clean pair).

use std::process::{Command, Output};

use mock_nexus::{MockNexus, Scenario};

const NXR: &str = env!("CARGO_BIN_EXE_nxr");

fn nxr(args: &[&str]) -> Output {
    let out = Command::new(NXR)
        .args(args)
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD")
        .output()
        .expect("nxr runs");
    if out.status.code() == Some(2) {
        eprintln!("misuse stderr: {}", String::from_utf8_lossy(&out.stderr));
    }
    out
}

/// The base URL of the audit repository: the seeded litter lives under `1.0.0/`.
fn repo_url(srv: &MockNexus) -> String {
    format!("{}repository/raw-main/1.0.0/", srv.base_url())
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn structural_mode_reports_every_litter_class() {
    let srv = MockNexus::start(Scenario::AuditLitter).expect("mock nexus starts");
    let out = nxr(&["audit", &repo_url(&srv)]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "findings are exit 1: {}",
        stdout(&out)
    );
    let s = stdout(&out);
    assert!(s.contains("orphan orphan/app.bin"), "orphan line: {s}");
    assert!(
        s.contains("dangling_marker dangling/app.bin"),
        "dangling line: {s}"
    );
    assert!(
        s.contains("unparseable_marker broken/app.bin"),
        "unparseable line: {s}"
    );
    // The clean pair is the control: flagged by nothing.
    assert!(
        !s.contains("clean/app.bin"),
        "clean pair must stay silent: {s}"
    );
    assert!(s.contains("findings: 3"), "the summary counts three: {s}");
}

#[test]
fn clean_prefix_exits_zero() {
    // The clean corner of the audit-litter repo is the control: a full pair
    // with an agreeing search fact must stay silent and exit 0. A repo with
    // no search API refuses the listing (exit 1, the enumeration class),
    // because an audit over an incomplete listing is not an audit.
    let srv = MockNexus::start(Scenario::AuditLitter).expect("mock nexus starts");
    let out = nxr(&["audit", &repo_url(&srv), "--prefix", "clean"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "clean prefix exits 0: {}",
        stdout(&out)
    );
    let s = stdout(&out);
    assert!(s.contains("findings: 0"), "zero findings: {s}");
}

#[test]
fn json_mode_emits_finding_events_and_summary() {
    let srv = MockNexus::start(Scenario::AuditLitter).expect("mock nexus starts");
    let out = nxr(&["--json", "audit", &repo_url(&srv)]);
    assert_eq!(out.status.code(), Some(1));
    let s = stdout(&out);
    assert!(s.contains(r#""event":"finding""#), "finding events: {s}");
    assert!(s.contains(r#""class":"orphan""#), "orphan class: {s}");
    assert!(s.contains(r#""event":"summary""#), "summary event: {s}");
    assert!(
        s.contains(r#""findings":3"#),
        "the summary counts the three structural findings: {s}"
    );
}

#[test]
fn sample_without_bytes_is_misuse() {
    let srv = MockNexus::start(Scenario::AuditLitter).expect("mock nexus starts");
    let out = nxr(&["audit", &repo_url(&srv), "--sample", "2"]);
    assert_eq!(out.status.code(), Some(2), "sample without bytes is misuse");
}

#[test]
fn byte_mode_verifies_the_clean_pair() {
    let srv = MockNexus::start(Scenario::AuditLitter).expect("mock nexus starts");
    let out = nxr(&["audit", &repo_url(&srv), "--bytes"]);
    // The litter classes stay (bytes mode adds nothing to them), and the byte
    // pass verifies the clean pair without flagging it: no byte_mismatch line.
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let s = stdout(&out);
    assert!(
        !s.contains("byte_mismatch"),
        "the clean pair must byte-verify: {s}"
    );
    assert!(
        s.contains("bytes checked 1"),
        "exactly one clean asset checked: {s}"
    );
}
