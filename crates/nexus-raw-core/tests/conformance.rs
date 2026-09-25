//! Conformance: the modules.md §7 invariant matrix, driven through the core
//! facade against the mock-nexus failure scenarios.

use std::path::Path;
use std::time::Duration;

use mock_nexus::{MockNexus, Scenario};
use nexus_raw_core::{
    creds::basic, model::sibling, Action, Claim, Config, Error, Event, Nxr, Summary,
};
use tempfile::TempDir;
use tokio::sync::mpsc;

const USER: &str = "ci";
const PASS: &str = "secret";
const VERSION: &str = "1.0.0";

fn config(mock: &MockNexus, auth: Option<String>) -> Config {
    Config {
        base: mock.base_url(),
        workers: 4,
        retry_attempts: 4,
        connect_timeout: Duration::from_secs(5),
        stall_timeout: Duration::from_secs(30),
        tls_insecure: false,
        auth,
    }
}

fn collect_events(rx: &mut mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

/// A local directory holding a complete artifact: bytes + canonical marker.
fn seed_complete(dir: &Path, name: &str, content: &[u8]) {
    std::fs::write(dir.join(name), content).unwrap();
    let marker = sibling::format_line(name, &nexus_raw_core::Digest::of_bytes(content));
    std::fs::write(dir.join(format!("{name}.sha256")), marker).unwrap();
}

fn claim(names: &[&str]) -> Claim {
    Claim {
        claim_version: 1,
        version: VERSION.to_owned(),
        artifacts: names
            .iter()
            .map(|n| nexus_raw_core::ArtifactName::parse(n).unwrap())
            .collect(),
    }
}

fn summary_of(events: &[Event]) -> Option<&Summary> {
    events.iter().rev().find_map(|e| match e {
        Event::Summary(s) => Some(s),
        _ => None,
    })
}

#[tokio::test]
async fn two_phase_up_recovers_after_partial_put() {
    let mock = MockNexus::start(Scenario::PartialPut {
        first_attempt_bytes: 64,
    })
    .unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", &[0xa; 4096]);
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    // A single call retries past the cut and finishes the upload.
    nxr.up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();
    let stored = mock.store_get("1.0.0/a.zip").unwrap();
    assert_eq!(stored, vec![0xa; 4096]);
    assert!(mock.store_get("1.0.0/a.zip.sha256").is_some());

    // The marker request strictly follows the bytes request of the same name.
    let log = mock.requests();
    let bytes_at = log
        .iter()
        .position(|r| r.method == "PUT" && r.path == "1.0.0/a.zip")
        .unwrap();
    let marker_at = log
        .iter()
        .position(|r| r.method == "PUT" && r.path == "1.0.0/a.zip.sha256")
        .unwrap();
    assert!(bytes_at < marker_at, "marker must follow bytes");
    drop(rx);
}

#[tokio::test]
async fn markerless_remote_is_re_uploaded() {
    let mock = MockNexus::start(Scenario::Markerless).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", b"hello");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    nxr.up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();
    let puts_after_first = mock.put_count("1.0.0/a.zip");
    assert_eq!(puts_after_first, 1);
    // The marker was accepted by the server but silently dropped.
    assert!(mock.store_get("1.0.0/a.zip.sha256").is_none());

    // The second up reports the missing marker and transfers again.
    let summary = nxr
        .up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();
    assert_eq!(summary.uploaded, 1);
    assert_eq!(mock.put_count("1.0.0/a.zip"), 2);
    let events = collect_events(&mut rx);
    let summary_event = summary_of(&events).unwrap();
    assert_eq!(summary_event.uploaded, 1);
    drop(rx);
}

#[tokio::test]
async fn foreign_marker_stops_up_and_down_without_overwrite() {
    let mock = MockNexus::start(Scenario::ForeignMarker).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", b"payload");
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    // First up succeeds: the client does not re-verify after PUT.
    nxr.up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();
    let puts_after_first = mock.put_count("1.0.0/a.zip");
    // The server stored the marker with a foreign digest.
    let marker = mock.store_get("1.0.0/a.zip.sha256").unwrap();
    assert!(marker.starts_with(b"0000"));

    // Second up must refuse on the digest divergence and never touch the bytes.
    let err = nxr
        .up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert_eq!(mock.put_count("1.0.0/a.zip"), puts_after_first);

    // Down of the diverged object refuses and writes nothing locally.
    let target = TempDir::new().unwrap();
    let err = nxr
        .down(target.path(), &claim(&["a.zip"]), None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Mismatch { .. }));
    assert!(!target.path().join("a.zip").exists());
    drop(rx);
}

#[tokio::test]
async fn single_call_recovers_through_flaky_and_dropped_connections() {
    let mock = MockNexus::start(Scenario::Flaky { first_failures: 2 }).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", b"resume-me");
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    nxr.up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();

    // Down through the same flaky server completes in one call too.
    let target = TempDir::new().unwrap();
    let summary = nxr
        .down(target.path(), &claim(&["a.zip"]), None, None)
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1);
    assert_eq!(
        std::fs::read(target.path().join("a.zip")).unwrap(),
        b"resume-me"
    );
}

#[tokio::test]
async fn claim_drift_refuses_and_keeps_remote_artifacts() {
    let mock = MockNexus::start(Scenario::ClaimDrift).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", b"one");
    seed_complete(local.path(), "b.zip", b"two");
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    nxr.up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();
    let artifact_puts_before = mock.put_count("1.0.0/a.zip");

    // The remote claim silently diverges from the local one.
    mock.enable_drift();
    let err = nxr
        .up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ClaimDrift { .. }));
    assert_eq!(mock.put_count("1.0.0/a.zip"), artifact_puts_before);

    // With the drift gone, re-uploading the same claim is a pure skip.
    mock.disable_drift();
    let summary = nxr
        .up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();
    assert_eq!(summary.uploaded, 0);
    assert_eq!(summary.skipped, 1);
}

#[tokio::test]
async fn auth_gates_every_request() {
    let mock = MockNexus::start(Scenario::Auth401 {
        user: USER.into(),
        pass: PASS.into(),
    })
    .unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", b"secret-artifact");
    let (tx, _rx) = mpsc::unbounded_channel();

    // No credentials: refused on the first probe.
    let nxr = Nxr::new(config(&mock, None), tx.clone()).unwrap();
    let err = nxr
        .up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Auth { .. }));
    assert_eq!(err.exit_code(), 3);

    // Wrong credentials: the same refusal.
    let wrong = format!("Basic {}", basic("ci", "wrong"));
    let nxr = Nxr::new(config(&mock, Some(wrong)), tx.clone()).unwrap();
    let err = nxr
        .up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Auth { .. }));

    // Correct credentials: the upload goes through.
    let right = format!("Basic {}", basic(USER, PASS));
    let nxr = Nxr::new(config(&mock, Some(right)), tx).unwrap();
    nxr.up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();
    assert_eq!(mock.store_get("1.0.0/a.zip").unwrap(), b"secret-artifact");
}

#[tokio::test]
async fn stalled_download_retries_then_refuses() {
    let mock = MockNexus::start(Scenario::Slow {
        chunk_delay_ms: 1500,
        chunk_size: 256,
    })
    .unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", &[7; 4096]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut cfg = config(&mock, None);
    cfg.stall_timeout = Duration::from_millis(400);
    cfg.retry_attempts = 2;
    let nxr = Nxr::new(cfg, tx).unwrap();

    // Publish through the slow server? No: stall applies to the download side,
    // and up of a markerless-remote path needs a complete remote. Seed the
    // remote store directly and fetch with the tiny stall timeout.
    nxr.up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();

    let target = TempDir::new().unwrap();
    let err = nxr
        .down(target.path(), &claim(&["a.zip"]), None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Transport { .. }));
    assert_eq!(err.exit_code(), 3);
    // The stalled name leaves nothing at the destination (§6.2.3).
    assert!(!target.path().join("a.zip").exists());
    drop(_rx);
}

#[tokio::test]
async fn down_cleans_orphans_and_completes() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", b"orphan-case");
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    nxr.up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();

    // A leftover temp file of a dead process: cleaned on the next down.
    let target = TempDir::new().unwrap();
    let orphan = target.path().join(".nxr-tmp-999999-0123456789abcdef.bytes");
    std::fs::write(&orphan, b"half-written").unwrap();

    nxr.down(target.path(), &claim(&["a.zip"]), None, None)
        .await
        .unwrap();
    assert!(!orphan.exists());
    assert_eq!(
        std::fs::read(target.path().join("a.zip")).unwrap(),
        b"orphan-case"
    );
    let marker = std::fs::read_to_string(target.path().join("a.zip.sha256")).unwrap();
    assert!(marker.ends_with("  a.zip\n"));
}

#[tokio::test]
async fn down_markerless_remote_writes_computed_marker() {
    let mock = MockNexus::start(Scenario::Markerless).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", b"no-marker-remote");
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    nxr.up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap();
    assert!(mock.store_get("1.0.0/a.zip.sha256").is_none());

    // Absent locally + Markerless remotely: fetch the bytes, compute the marker.
    let target = TempDir::new().unwrap();
    nxr.down(target.path(), &claim(&["a.zip"]), None, None)
        .await
        .unwrap();
    let marker = std::fs::read_to_string(target.path().join("a.zip.sha256")).unwrap();
    let parsed = sibling::parse_line(&marker).unwrap();
    assert_eq!(
        parsed.digest.as_str(),
        nexus_raw_core::Digest::of_bytes(b"no-marker-remote").as_str()
    );
}

#[tokio::test]
async fn missing_nowhere_refuses_down() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let target = TempDir::new().unwrap();
    let err = nxr
        .down(target.path(), &claim(&["ghost.zip"]), None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Missing { .. }));
    assert_eq!(err.exit_code(), 1);
}

#[tokio::test]
async fn up_precheck_refuses_incomplete_local_build() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    // Bytes without a marker: a build error, not something to publish (§5.2).
    std::fs::write(local.path().join("a.zip"), b"markerless").unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let err = nxr
        .up(local.path(), &claim(&["a.zip"]), None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Incomplete { .. }));
    assert_eq!(err.exit_code(), 1);
}

#[tokio::test]
async fn verify_reports_local_state_without_network() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "ok.zip", b"good");
    std::fs::write(local.path().join("bad.zip"), b"bad").unwrap();
    std::fs::write(
        local.path().join("bad.zip.sha256"),
        sibling::format_line("bad.zip", &nexus_raw_core::Digest::of_bytes(b"other")),
    )
    .unwrap();

    // verify never touches the base: a dummy URL proves no request happens.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let summary = nxr.verify(local.path(), &claim(&["ok.zip"])).await.unwrap();
    assert_eq!(summary.skipped, 1);

    let err = nxr
        .verify(local.path(), &claim(&["ok.zip", "bad.zip"]))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Incomplete { .. }));
    let events = collect_events(&mut rx);
    let last = summary_of(&events).unwrap();
    assert_eq!(last.failed, vec!["bad.zip".to_string()]);
    assert!(
        mock.requests().is_empty(),
        "verify must not use the network"
    );
}

#[tokio::test]
async fn pointer_is_forward_only_with_if_newer() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let outcome = nxr.point("latest", "1.4.0", false).await.unwrap();
    assert!(matches!(
        outcome,
        nexus_raw_core::PointOutcome::Written { from: None }
    ));

    // Forward: written. Backward with --if-newer: skipped.
    let outcome = nxr.point("latest", "1.5.0", true).await.unwrap();
    assert!(matches!(
        outcome,
        nexus_raw_core::PointOutcome::Written { .. }
    ));
    let outcome = nxr.point("latest", "1.4.9", true).await.unwrap();
    assert_eq!(
        outcome,
        nexus_raw_core::PointOutcome::Skipped {
            current: "1.5.0".into()
        }
    );

    // Without --if-newer a backward move is explicit and allowed.
    let outcome = nxr.point("latest", "1.0.0", false).await.unwrap();
    assert!(matches!(
        outcome,
        nexus_raw_core::PointOutcome::Written { .. }
    ));
    assert_eq!(mock.store_get("latest").unwrap(), b"1.0.0\n".to_vec());
}

#[tokio::test]
async fn diff_plan_is_deterministic_and_events_carry_lists() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "have.zip", b"already-there");
    // Remote gets the same bytes + marker for have.zip through a first up.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    nxr.up(local.path(), &claim(&["have.zip"]), None)
        .await
        .unwrap();

    // Now have.zip is a Skip, missing.zip a Download (for down) / Upload pair.
    seed_complete(local.path(), "have.zip", b"already-there");
    let actions = nxr
        .diff(local.path(), &claim(&["have.zip", "missing.zip"]))
        .await;
    // missing.zip exists nowhere: the diff refuses with Missing.
    assert!(actions.is_err());

    // A purely local-up view: everything complete and already uploaded → all Skip.
    let actions = nxr.diff(local.path(), &claim(&["have.zip"])).await.unwrap();
    assert!(matches!(&actions[..], [Action::Skip { .. }]));
    let events = collect_events(&mut rx);
    assert!(events.iter().any(|e| matches!(e, Event::Plan { .. })));
}
