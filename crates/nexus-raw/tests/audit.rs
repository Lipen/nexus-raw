//! CLI conformance: `nxr audit` against the `audit-litter` scenario.
//!
//! Covers the structural classes (orphan, dangling marker, unparseable marker), the clean pair never flagged, and the exit-code verdict.
//! It also covers the two-mode contract: the structural pass sees no byte corruption, the byte pass verifies the clean pair.

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
    let out = nxr(&["audit", &repo_url(&srv), "--prefix", "clean/"]);
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

/// The positive bitrot case: corrupt bytes under an intact marker are a byte mismatch, exit 1.
#[test]
fn byte_mode_flags_a_corrupted_asset() {
    let srv = MockNexus::start(Scenario::AuditLitter).expect("mock nexus starts");
    // Replace the clean object's bytes, keep its marker: the byte pass must see it.
    srv.insert("repository/raw-main/1.0.0/clean/app.bin", b"bitrot");

    let out = nxr(&["audit", &repo_url(&srv), "--bytes", "--prefix", "clean/"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "bitrot is exit 1: {}",
        stdout(&out)
    );
    let s = stdout(&out);
    assert!(
        s.contains("byte_mismatch") && s.contains("clean/app.bin"),
        "the corrupted asset is named: {s}"
    );
    assert!(s.contains("bytes checked 1"), "one asset checked: {s}");
}

/// `--name` narrows the audit to the named entries, and only those.
#[test]
fn name_filter_limits_the_findings() {
    let srv = MockNexus::start(Scenario::AuditLitter).expect("mock nexus starts");

    let clean = nxr(&["audit", &repo_url(&srv), "--name", "clean/app.bin"]);
    assert_eq!(
        clean.status.code(),
        Some(0),
        "the clean name is silent: {}",
        stdout(&clean)
    );
    assert!(stdout(&clean).contains("findings: 0"));

    let orphan = nxr(&["audit", &repo_url(&srv), "--name", "orphan/app.bin"]);
    assert_eq!(
        orphan.status.code(),
        Some(1),
        "the orphan is a finding: {}",
        stdout(&orphan)
    );
    let s = stdout(&orphan);
    assert!(
        s.contains("orphan orphan/app.bin"),
        "the named finding: {s}"
    );
    assert!(!s.contains("dangling"), "the other names are out: {s}");
}

/// The audit's `--workers` is the shared worker budget: zero is misuse, not a silent default.
#[test]
fn audit_workers_zero_is_misuse() {
    let srv = MockNexus::start(Scenario::AuditLitter).expect("mock nexus starts");
    let out = nxr(&["audit", &repo_url(&srv), "--workers", "0"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--workers"),
        "the refusal names the flag: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn hex_digest(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn marker_line(name: &str, bytes: &[u8]) -> String {
    format!("{}  {name}\n", hex_digest(bytes))
}

/// `--sample N` checks exactly N of the eligible assets, deterministically.
#[test]
fn sample_checks_the_requested_count() {
    let srv = MockNexus::start(Scenario::Atomic).expect("mock nexus starts");
    let mut items = Vec::new();
    for i in 0..4 {
        let name = format!("s{i}.bin");
        let path = format!("1.0.0/{name}");
        let body = format!("payload-{i}").into_bytes();
        let digest = hex_digest(&body);
        srv.insert(&format!("repository/raw-main/{path}"), &body);
        srv.insert(
            &format!("repository/raw-main/{path}.sha256"),
            marker_line(&name, &body).as_bytes(),
        );
        items.push(serde_json::json!({"path": path, "checksum": {"sha256": digest.clone()}}));
        items.push(
            serde_json::json!({"path": format!("{path}.sha256"), "checksum": {"sha256": digest}}),
        );
    }
    srv.insert(
        "service/rest/v1/search/assets",
        serde_json::json!({"items": items}).to_string().as_bytes(),
    );

    let base = format!("{}repository/raw-main/1.0.0/", srv.base_url());
    let out = nxr(&["audit", &base, "--bytes", "--sample", "2"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("bytes checked 2"),
        "the sample checks two: {}",
        stdout(&out)
    );

    // A second run picks the same stride: the selection is deterministic.
    let again = nxr(&["audit", &base, "--bytes", "--sample", "2"]);
    assert_eq!(again.status.code(), Some(0));
    assert!(stdout(&again).contains("bytes checked 2"));
}
