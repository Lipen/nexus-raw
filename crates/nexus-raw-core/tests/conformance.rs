//! Conformance: the v0.3 invariant matrix, driven through the core facade
//! against the mock-nexus failure scenarios.

use std::path::Path;
use std::time::Duration;

use mock_nexus::{MockNexus, Outcome, Scenario};
use nexus_raw_core::sync::down::part_path;
use nexus_raw_core::{
    model::sibling, ArtifactName, Config, Digest, Enumeration, Error, Event, Manifest, Mode, Nxr,
    Summary,
};
use tempfile::TempDir;
use tokio::sync::mpsc;

const USER: &str = "ci";
const PASS: &str = "secret";
const VERSION: &str = "1.0.0";
const CONTENT: &[u8] = b"payload-0123456789";

fn config(mock: &MockNexus, auth: Option<String>) -> Config {
    Config {
        base: dir_url(mock),
        workers: 4,
        retry_attempts: 4,
        connect_timeout: Duration::from_secs(5),
        stall_timeout: Duration::from_secs(30),
        tls_insecure: false,
        auth,
    }
}

/// The invocation base: the version directory URL inside the mock store.
fn dir_url(mock: &MockNexus) -> String {
    format!("{}{VERSION}/", mock.base_url())
}

fn collect_events(rx: &mut mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

fn summary_of(events: &[Event]) -> Option<&Summary> {
    events.iter().rev().find_map(|e| match e {
        Event::Summary(s) => Some(s),
        _ => None,
    })
}

/// A local directory holding one complete artifact: bytes + canonical marker.
fn seed_complete(dir: &Path, name: &str, content: &[u8]) {
    let bytes = dir.join(name);
    std::fs::create_dir_all(bytes.parent().unwrap()).unwrap();
    std::fs::write(&bytes, content).unwrap();
    let marker = sibling::format_line(name, &Digest::of_bytes(content));
    std::fs::write(dir.join(format!("{name}.sha256")), marker).unwrap();
}

fn names(list: &[&str]) -> Vec<ArtifactName> {
    list.iter()
        .map(|n| ArtifactName::parse(n).unwrap())
        .collect()
}

// ---------------------------------------------------------------- up

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
    nxr.up(local.path(), None, true, None, None).await.unwrap();
    assert_eq!(
        mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        vec![0xa; 4096]
    );
    assert!(mock.store_get(&format!("{VERSION}/a.zip.sha256")).is_some());

    // The marker request strictly follows the bytes request of the same name.
    let log = mock.requests();
    let bytes_at = log
        .iter()
        .position(|r| r.method == "PUT" && r.path == format!("{VERSION}/a.zip"))
        .unwrap();
    let marker_at = log
        .iter()
        .position(|r| r.method == "PUT" && r.path == format!("{VERSION}/a.zip.sha256"))
        .unwrap();
    assert!(bytes_at < marker_at, "marker must follow bytes");
    drop(rx);
}

#[tokio::test]
async fn stalled_upload_fails_within_the_stall_window() {
    // freeze-upload holds the connection after the head: the attempt-level
    // stall watchdog must surface a retryable transport failure instead of
    // hanging on a socket nobody drains. Regression: the stall error once
    // had to travel through the very channel it was reporting about, so a
    // full channel meant the timeout never surfaced.
    let mock = MockNexus::start(Scenario::FreezeUpload).unwrap();
    let local = TempDir::new().unwrap();
    // Big enough that kernel socket buffers and the body channel fill: the
    // write side must actually feel the freeze.
    let big = vec![0x5au8; 16 * 1024 * 1024];
    std::fs::write(local.path().join("big.zip"), &big).unwrap();

    let mut cfg = config(&mock, None);
    cfg.stall_timeout = Duration::from_secs(1);
    cfg.retry_attempts = 2;
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).unwrap();

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        nxr.up(local.path(), None, true, None, None),
    )
    .await;
    let elapsed = started.elapsed();
    let result = result.unwrap_or_else(|_| panic!("upload hung past 30s under a frozen server"));
    let error = result.unwrap_err();
    assert_eq!(error.exit_code(), 3, "expected transport, got {error}");
    assert!(
        elapsed >= Duration::from_secs(1),
        "failed too fast to have watched the stall: {elapsed:?}"
    );
    let puts = mock.requests().iter().filter(|r| r.method == "PUT").count();
    assert!(puts >= 2, "expected a retry, saw {puts} PUT attempts");
}

#[tokio::test]
async fn divergent_complete_refusal_is_wire_covered() {
    // The never-overwrite invariant, driven through the wire: a complete
    // remote artifact with a different digest refuses the whole up and the
    // remote bytes stay untouched. Regression net: a revert to overwrite
    // would pass the rest of the suite green — only unit tests covered the
    // verdict before this test.
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let remote = b"remote bytes";
    mock.insert(&format!("{VERSION}/a.zip"), remote);
    mock.insert(
        &format!("{VERSION}/a.zip.sha256"),
        sibling::format_line("a.zip", &Digest::of_bytes(remote)).as_bytes(),
    );
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", b"local bytes");
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let err = nxr
        .up(local.path(), None, true, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 1, "divergence is a data error, got {err}");
    assert_eq!(
        mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        remote,
        "the remote bytes must survive the refused up"
    );
}

#[tokio::test]
async fn sizeless_head_refuses_instead_of_overwriting() {
    // The proxy case: a 200 without Content-Length used to classify as
    // Absent, so the upload skipped the digest comparison and overwrote.
    // The object is present but unverifiable: refusal, not overwrite.
    let mock = MockNexus::start(Scenario::Sizeless).unwrap();
    let remote = b"remote bytes";
    mock.insert(&format!("{VERSION}/a.zip"), remote);
    mock.insert(
        &format!("{VERSION}/a.zip.sha256"),
        sibling::format_line("a.zip", &Digest::of_bytes(remote)).as_bytes(),
    );
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", b"local bytes");
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let err = nxr
        .up(local.path(), None, true, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 1, "unverifiable remote refuses, got {err}");
    assert_eq!(
        mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        remote,
        "the remote bytes must survive the refused up"
    );
    assert!(
        mock.put_count(&format!("{VERSION}/a.zip")) == 0,
        "nothing may be PUT behind a sizeless HEAD"
    );
}

#[tokio::test]
async fn drop_connection_is_retried_through_the_client() {
    // DropConnection resets the first request per path. Driven through the
    // core client (it used to run only against the mock's own unit tests):
    // the first request is reset on the wire, the retry succeeds, the
    // artifact lands complete.
    let mock = MockNexus::start(Scenario::DropConnection).unwrap();
    mock.insert(&format!("{VERSION}/a.zip"), CONTENT);
    mock.insert(
        &format!("{VERSION}/a.zip.sha256"),
        sibling::format_line("a.zip", &Digest::of_bytes(CONTENT)).as_bytes(),
    );
    let local = TempDir::new().unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let summary = nxr
        .down(
            local.path(),
            Enumeration::Names(names(&["a.zip"])),
            false,
            None,
        )
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1);
    assert_eq!(std::fs::read(local.path().join("a.zip")).unwrap(), CONTENT);
    assert!(local.path().join("a.zip.sha256").is_file());
    assert!(
        matches!(
            mock.requests().first().map(|r| &r.outcome),
            Some(Outcome::Reset)
        ),
        "the first request on the first path must have been reset"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_part_is_never_followed_on_resume() {
    // The part name is derived from the server-controlled object
    // name, so a pre-placed symlink at the part path must fail the write
    // (O_NOFOLLOW), never become a write gadget into the decoy.
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    mock.insert(&format!("{VERSION}/a.zip"), CONTENT);
    mock.insert(
        &format!("{VERSION}/a.zip.sha256"),
        sibling::format_line("a.zip", &Digest::of_bytes(CONTENT)).as_bytes(),
    );
    let local = TempDir::new().unwrap();
    let decoy = local.path().join("decoy.txt");
    std::fs::write(&decoy, b"decoy bytes").unwrap();
    let a = names(&["a.zip"]).remove(0);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&decoy, part_path(local.path(), &a)).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let err = nxr
        .down(local.path(), Enumeration::Names(vec![a]), false, None)
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 1, "the refused write is a data error");
    assert_eq!(
        std::fs::read(&decoy).unwrap(),
        b"decoy bytes",
        "the decoy must survive untouched"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_marker_is_never_followed_on_write() {
    // Same class as the part case, one stage later: bytes land fine, the
    // local marker write must refuse to follow a symlink.
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    mock.insert(&format!("{VERSION}/a.zip"), CONTENT);
    mock.insert(
        &format!("{VERSION}/a.zip.sha256"),
        sibling::format_line("a.zip", &Digest::of_bytes(CONTENT)).as_bytes(),
    );
    let local = TempDir::new().unwrap();
    let decoy = local.path().join("decoy.txt");
    std::fs::write(&decoy, b"decoy bytes").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&decoy, local.path().join("a.zip.sha256")).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let err = nxr
        .down(
            local.path(),
            Enumeration::Names(names(&["a.zip"])),
            false,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 1, "the refused write is a data error");
    assert_eq!(
        std::fs::read(&decoy).unwrap(),
        b"decoy bytes",
        "the decoy must survive untouched"
    );
    assert_eq!(
        std::fs::read(local.path().join("a.zip")).unwrap(),
        CONTENT,
        "the verified bytes still land"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_local_marker_is_never_followed_on_up() {
    // The up side of the rule: auto-generated markers go through the same
    // NOFOLLOW open as the download side. A symlink at the marker path
    // fails the run before anything is sent; the decoy stays intact.
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    std::fs::write(local.path().join("a.zip"), CONTENT).unwrap();
    let decoy = local.path().join("decoy.txt");
    std::fs::write(&decoy, b"decoy bytes").unwrap();
    std::os::unix::fs::symlink(&decoy, local.path().join("a.zip.sha256")).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let err = nxr
        .up(local.path(), None, true, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 1, "the refused write is a data error");
    assert_eq!(
        std::fs::read(&decoy).unwrap(),
        b"decoy bytes",
        "the decoy must survive untouched"
    );
    assert_eq!(
        mock.put_count(&format!("{VERSION}/a.zip")),
        0,
        "markers are generated before any transfer starts"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn oversized_small_get_refuses_with_the_cap() {
    // A manifest is a "small" GET: past the 16 MiB cap the read refuses
    // (exit 2) instead of slurping an unbounded body into memory.
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    mock.insert(
        &format!("{VERSION}/manifest.json"),
        &vec![0u8; 16 * 1024 * 1024 + 1],
    );
    let local = TempDir::new().unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let err = nxr.manifest_at_base().await.unwrap_err();
    assert_eq!(err.exit_code(), 2, "the cap is a misuse refusal, got {err}");
    assert!(
        err.to_string().contains("small-object cap"),
        "the message names the cap: {err}"
    );
}

#[tokio::test]
async fn up_generates_markers_by_default() {
    // The plain-mode regression: a directory without any .sha256 file still
    // produces Complete remote objects (§5.2 markers-on-by-default).
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    std::fs::write(local.path().join("a.zip"), CONTENT).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    nxr.up(local.path(), None, true, None, None).await.unwrap();
    let stored = mock.store_get(&format!("{VERSION}/a.zip")).unwrap();
    assert_eq!(stored, CONTENT);
    let marker = mock.store_get(&format!("{VERSION}/a.zip.sha256")).unwrap();
    let expected = sibling::format_line("a.zip", &Digest::of_bytes(CONTENT));
    assert_eq!(String::from_utf8(marker).unwrap(), expected);

    // The local directory gained the sibling too: the dir is now self-complete.
    assert!(local.path().join("a.zip.sha256").is_file());
}

#[tokio::test]
async fn up_no_sha_skips_markers() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    std::fs::write(local.path().join("a.zip"), CONTENT).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    nxr.up(local.path(), None, false, None, None).await.unwrap();
    assert!(mock.store_get(&format!("{VERSION}/a.zip")).is_some());
    assert!(mock.store_get(&format!("{VERSION}/a.zip.sha256")).is_none());
    assert!(!local.path().join("a.zip.sha256").exists());
}

#[tokio::test]
async fn markerless_remote_is_re_uploaded() {
    let mock = MockNexus::start(Scenario::Markerless).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", b"hello");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    nxr.up(local.path(), None, true, None, None).await.unwrap();
    assert_eq!(mock.put_count(&format!("{VERSION}/a.zip")), 1);
    // The marker was accepted by the server but silently dropped.
    assert!(mock.store_get(&format!("{VERSION}/a.zip.sha256")).is_none());

    // The second up re-sends: the remote copy is not provably complete.
    let summary = nxr.up(local.path(), None, true, None, None).await.unwrap();
    assert_eq!(summary.uploaded, 1);
    assert_eq!(mock.put_count(&format!("{VERSION}/a.zip")), 2);
    let events = collect_events(&mut rx);
    let summary_event = summary_of(&events).unwrap();
    assert_eq!(summary_event.uploaded, 1);
}

#[tokio::test]
async fn second_up_is_all_skip() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    nxr.up(local.path(), None, true, None, None).await.unwrap();
    let puts = mock.put_count(&format!("{VERSION}/a.zip"));
    let summary = nxr.up(local.path(), None, true, None, None).await.unwrap();
    assert_eq!(summary.uploaded, 0);
    assert_eq!(summary.skipped, 1);
    assert_eq!(mock.put_count(&format!("{VERSION}/a.zip")), puts);
}

#[tokio::test]
async fn broken_local_marker_refuses_up() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    std::fs::write(local.path().join("a.zip"), CONTENT).unwrap();
    // A marker that does not match the bytes.
    let foreign = sibling::format_line("a.zip", &Digest::zero());
    std::fs::write(local.path().join("a.zip.sha256"), foreign).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let err = nxr
        .up(local.path(), None, true, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Mismatch { .. }), "got {err:?}");
    assert_eq!(err.exit_code(), 1);
    // Nothing was written remotely.
    assert!(mock.store_get(&format!("{VERSION}/a.zip")).is_none());
}

#[tokio::test]
async fn up_manifest_missing_local_name_is_data_error() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    std::fs::write(local.path().join("a.zip"), CONTENT).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let manifest = Manifest {
        names: names(&["a.zip", "ghost.bin"]),
    };
    let err = nxr
        .up(local.path(), Some(manifest.names), true, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Missing { .. }), "got {err:?}");
    assert_eq!(err.exit_code(), 1);
}

#[tokio::test]
async fn single_call_recovers_through_flaky() {
    let mock = MockNexus::start(Scenario::Flaky { first_failures: 2 }).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    nxr.up(local.path(), None, true, None, None).await.unwrap();
    assert_eq!(
        mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        CONTENT
    );
}

#[tokio::test]
async fn empty_dir_refuses_up() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let err = nxr
        .up(local.path(), None, true, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Misuse(_)));
    assert_eq!(err.exit_code(), 2);
}

// ---------------------------------------------------------------- down

#[tokio::test]
async fn down_fetches_and_writes_local_marker() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    // Seed the remote directly: put a dir up.
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    nxr.up(src.path(), None, true, None, None).await.unwrap();

    let dst = TempDir::new().unwrap();
    let summary = nxr
        .down(
            dst.path(),
            Enumeration::Names(names(&["a.zip"])),
            true,
            None,
        )
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1);
    assert_eq!(std::fs::read(dst.path().join("a.zip")).unwrap(), CONTENT);
    let marker = std::fs::read_to_string(dst.path().join("a.zip.sha256")).unwrap();
    assert_eq!(
        marker,
        sibling::format_line("a.zip", &Digest::of_bytes(CONTENT))
    );
}

#[tokio::test]
async fn down_resumes_from_part_with_range() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    nxr.up(src.path(), None, true, None, None).await.unwrap();

    // Pre-seed the part file with the first bytes, as an interrupted run left it.
    let dst = TempDir::new().unwrap();
    let name = ArtifactName::parse("a.zip").unwrap();
    let part = part_path(dst.path(), &name);
    std::fs::write(&part, &CONTENT[..5]).unwrap();

    let summary = nxr
        .down(
            dst.path(),
            Enumeration::Names(names(&["a.zip"])),
            false,
            None,
        )
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1);
    // The server answered 206 Partial Content for the resumed object.
    let hit = mock.requests().iter().any(|r| {
        r.method == "GET"
            && r.path == format!("{VERSION}/a.zip")
            && matches!(r.outcome, Outcome::Status(206))
    });
    assert!(
        hit,
        "expected a 206 range response, log: {:?}",
        mock.requests()
    );
    assert_eq!(std::fs::read(dst.path().join("a.zip")).unwrap(), CONTENT);
    // The part file was consumed by the rename.
    assert!(!part.exists());
}

#[tokio::test]
async fn down_resume_of_complete_part_finalizes_without_refetch() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    nxr.up(src.path(), None, true, None, None).await.unwrap();

    // The crash edge: the process died after the download finished but
    // before the rename, so the part already holds the whole object.
    let dst = TempDir::new().unwrap();
    let name = ArtifactName::parse("a.zip").unwrap();
    let part = part_path(dst.path(), &name);
    std::fs::write(&part, CONTENT).unwrap();

    let summary = nxr
        .down(
            dst.path(),
            Enumeration::Names(names(&["a.zip"])),
            false,
            None,
        )
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1);
    assert_eq!(std::fs::read(dst.path().join("a.zip")).unwrap(), CONTENT);
    assert!(!part.exists());
    // The server answered 416 (range already satisfied), never 206.
    let resumed_206 = mock.requests().iter().any(|r| {
        r.method == "GET"
            && r.path == format!("{VERSION}/a.zip")
            && matches!(r.outcome, Outcome::Status(206))
    });
    assert!(
        !resumed_206,
        "a complete part must not trigger a range fetch"
    );
}

#[tokio::test]
async fn down_auto_enumerates_through_manifest_convention() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    // No manifest yet: the conventional source is absent.
    assert!(nxr.manifest_at_base().await.unwrap().is_none());

    // A directory that carries manifest.json publishes it like any artifact.
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    std::fs::write(
        src.path().join("manifest.json"),
        br#"{"artifacts":["a.zip","manifest.json"]}"#,
    )
    .unwrap();
    nxr.up(src.path(), None, true, None, None).await.unwrap();

    let manifest = nxr.manifest_at_base().await.unwrap().unwrap();
    assert_eq!(manifest.names.len(), 2);
    let dst = TempDir::new().unwrap();
    let summary = nxr
        .down(dst.path(), Enumeration::Manifest(manifest), false, None)
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 2);
    assert_eq!(std::fs::read(dst.path().join("a.zip")).unwrap(), CONTENT);
}

#[tokio::test]
async fn down_digest_mismatch_refuses_and_drops_part() {
    let mock = MockNexus::start(Scenario::ForeignMarker).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    nxr.up(src.path(), None, true, None, None).await.unwrap();

    let dst = TempDir::new().unwrap();
    let err = nxr
        .down(
            dst.path(),
            Enumeration::Names(names(&["a.zip"])),
            true,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Mismatch { .. }), "got {err:?}");
    // The diverging part never survives: no bytes, no marker at the destination.
    assert!(!dst.path().join("a.zip").exists());
    assert!(!dst.path().join("a.zip.sha256").exists());
    assert!(
        part_path(dst.path(), &ArtifactName::parse("a.zip").unwrap())
            .metadata()
            .is_err()
    );
}

#[tokio::test]
async fn down_missing_name_is_data_error() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let dst = TempDir::new().unwrap();
    let err = nxr
        .down(
            dst.path(),
            Enumeration::Names(names(&["ghost.bin"])),
            true,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Missing { .. }), "got {err:?}");
}

#[tokio::test]
async fn down_markerless_remote_writes_computed_marker() {
    // Markerless remote: bytes without a marker. Down fetches them and
    // computes the marker locally (§5.2). Without a sibling there is nothing
    // to verify a resume against, so an existing part is ignored entirely
    // (§5.1): the name downloads from zero, however stale the part is.
    let mock = MockNexus::start(Scenario::Markerless).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    nxr.up(src.path(), None, false, None, None).await.unwrap(); // --no-sha

    let dst = TempDir::new().unwrap();
    let name = ArtifactName::parse("a.zip").unwrap();
    let part = part_path(dst.path(), &name);
    std::fs::write(&part, b"stale-bytes-that-must-never-survive").unwrap();
    nxr.down(
        dst.path(),
        Enumeration::Names(names(&["a.zip"])),
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(dst.path().join("a.zip")).unwrap(),
        CONTENT,
        "the stale part never hybridizes into the artifact"
    );
    let marker = std::fs::read_to_string(dst.path().join("a.zip.sha256")).unwrap();
    assert_eq!(
        marker,
        sibling::format_line("a.zip", &Digest::of_bytes(CONTENT))
    );
}

// ---------------------------------------------------------------- auth, stall

#[tokio::test]
async fn auth_gates_every_request() {
    let mock = MockNexus::start(Scenario::Auth401 {
        user: USER.to_owned(),
        pass: PASS.to_owned(),
    })
    .unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    let (tx, _rx) = mpsc::unbounded_channel();

    // Without credentials: Auth, exit 3, nothing stored.
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let err = nxr
        .up(local.path(), None, true, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Auth { .. }), "got {err:?}");
    assert_eq!(err.exit_code(), 3);
    assert!(mock.store_get(&format!("{VERSION}/a.zip")).is_none());

    // With credentials: through on the first try.
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(
        config(
            &mock,
            Some(format!(
                "Basic {}",
                nexus_raw_core::creds::basic(USER, PASS)
            )),
        ),
        tx,
    )
    .unwrap();
    nxr.up(local.path(), None, true, None, None).await.unwrap();
    assert_eq!(
        mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        CONTENT
    );
}

#[tokio::test]
async fn stalled_download_retries_then_refuses() {
    let mock = MockNexus::start(Scenario::Slow {
        chunk_delay_ms: 300,
        chunk_size: 8,
    })
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(
        Config {
            stall_timeout: Duration::from_millis(150),
            ..config(&mock, None)
        },
        tx,
    )
    .unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    nxr.up(src.path(), None, true, None, None).await.unwrap();

    let dst = TempDir::new().unwrap();
    let err = nxr
        .down(
            dst.path(),
            Enumeration::Names(names(&["a.zip"])),
            true,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Transport { .. }), "got {err:?}");
    assert_eq!(err.exit_code(), 3);
    assert!(!dst.path().join("a.zip").exists());
}

// ---------------------------------------------------------------- verify, diff, channel

#[tokio::test]
async fn verify_reports_local_state_without_network() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    std::fs::write(local.path().join("loose.bin"), b"no marker").unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    // The complete name verifies on its own.
    let summary = nxr
        .verify(local.path(), Some(names(&["a.zip"])))
        .await
        .unwrap();
    assert_eq!(summary.skipped, 1);

    // A markerless file is not complete: strict verdict.
    let err = nxr.verify(local.path(), None).await.unwrap_err();
    assert!(
        matches!(&err, Error::Incomplete { names } if names == &["loose.bin".to_owned()]),
        "got {err:?}"
    );

    // Tampered bytes join the failure list.
    let bytes = local.path().join("a.zip");
    let mut data = std::fs::read(&bytes).unwrap();
    data[0] ^= 0xff;
    std::fs::write(&bytes, data).unwrap();
    let err = nxr.verify(local.path(), None).await.unwrap_err();
    assert!(
        matches!(&err, Error::Incomplete { names } if names.contains(&"a.zip".to_owned())),
        "got {err:?}"
    );
    let events = collect_events(&mut rx);
    let s = summary_of(&events).unwrap();
    assert_eq!(s.failed.len(), 2);
}

#[tokio::test]
async fn diff_plan_is_deterministic_and_events_carry_lists() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    seed_complete(local.path(), "sub/b.txt", b"nested");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let plan = nxr
        .diff(local.path(), names(&["a.zip", "sub/b.txt"]), Mode::Up, true)
        .await
        .unwrap();
    let kinds: Vec<String> = plan
        .iter()
        .map(|a| match a {
            nexus_raw_core::Action::Upload { name, .. } => format!("upload {name}"),
            nexus_raw_core::Action::Skip { name, .. } => format!("skip {name}"),
            nexus_raw_core::Action::Download { name, .. } => format!("download {name}"),
        })
        .collect();
    assert_eq!(kinds, ["upload a.zip", "upload sub/b.txt"]);

    // The same plan through up, observed as a plan event.
    nxr.up(local.path(), None, true, None, None).await.unwrap();
    let events = collect_events(&mut rx);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Plan { upload, .. } if upload.len() == 2)));
}

#[tokio::test]
async fn channel_set_get_and_forward_guard() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let url = format!("{}latest", mock.base_url());

    assert!(nxr.channel_get(&url).await.unwrap().is_none());
    let out = nxr.channel_set(&url, "1.4.0", false).await.unwrap();
    assert!(matches!(
        out,
        nexus_raw_core::ChannelOutcome::Written { from: None }
    ));
    assert_eq!(nxr.channel_get(&url).await.unwrap().unwrap(), "1.4.0");
    // The stored file is exactly the token line.
    assert_eq!(mock.store_get("latest").unwrap(), b"1.4.0\n".to_vec());

    // Forward-only: an older token never lands.
    let out = nxr.channel_set(&url, "1.2.0", true).await.unwrap();
    assert!(matches!(
        out,
        nexus_raw_core::ChannelOutcome::Skipped { .. }
    ));
    assert_eq!(nxr.channel_get(&url).await.unwrap().unwrap(), "1.4.0");

    // Without the guard anything goes.
    let out = nxr.channel_set(&url, "1.2.0", false).await.unwrap();
    assert!(matches!(
        out,
        nexus_raw_core::ChannelOutcome::Written { .. }
    ));
    assert_eq!(nxr.channel_get(&url).await.unwrap().unwrap(), "1.2.0");
}

#[tokio::test]
async fn get_primitive_resumes_with_range() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    nxr.up(src.path(), None, true, None, None).await.unwrap();
    let url = format!("{}a.zip", dir_url(&mock));

    let out = TempDir::new().unwrap();
    let target = out.path().join("a.zip");
    let part = out.path().join("a.zip.part");
    std::fs::write(&part, &CONTENT[..3]).unwrap();

    let outcome = nxr.get(&url, Some(target.clone()), true).await.unwrap();
    assert_eq!(outcome.resumed_from, 3);
    assert_eq!(outcome.size, CONTENT.len() as u64);
    assert_eq!(std::fs::read(&target).unwrap(), CONTENT);
    assert!(!part.exists());
    let hit = mock.requests().iter().any(|r| {
        r.method == "GET"
            && r.path == format!("{VERSION}/a.zip")
            && matches!(r.outcome, Outcome::Status(206))
    });
    assert!(hit, "expected a 206 range response");
}

#[tokio::test]
async fn down_stale_part_self_heals_without_a_flag() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    nxr.up(src.path(), None, true, None, None).await.unwrap();

    // The part holds bytes of an object that no longer matches the sibling:
    // an interrupted download of an older remote version.
    let dst = TempDir::new().unwrap();
    let name = ArtifactName::parse("a.zip").unwrap();
    let part = part_path(dst.path(), &name);
    std::fs::write(&part, b"stale-prefix-of-an-old-object").unwrap();

    let summary = nxr
        .down(
            dst.path(),
            Enumeration::Names(names(&["a.zip"])),
            false,
            None,
        )
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1);
    assert_eq!(std::fs::read(dst.path().join("a.zip")).unwrap(), CONTENT);
    assert!(!part.exists());
    // The restart went from zero: two fetches, the last one a clean 200.
    // The first may answer 416 (stale prefix longer than the object) or 206.
    let log = mock.requests();
    let gets: Vec<_> = log
        .iter()
        .filter(|r| r.method == "GET" && r.path == format!("{VERSION}/a.zip"))
        .collect();
    assert_eq!(gets.len(), 2, "restart means two fetches: {gets:?}");
    assert!(
        matches!(gets[1].outcome, Outcome::Status(200)),
        "the clean restart ends in a full 200: {gets:?}"
    );
}

#[tokio::test]
async fn up_claim_first_puts_the_claim_before_any_payload() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "b.zip", CONTENT);
    seed_complete(src.path(), "a.zip", CONTENT);
    seed_complete(
        src.path(),
        "manifest.json",
        b"{\"artifacts\": [\"a.zip\", \"b.zip\"]}",
    );

    let claim = ArtifactName::parse("manifest.json").unwrap();
    nxr.up(src.path(), None, true, Some(claim), None)
        .await
        .unwrap();

    let log = mock.requests();
    let puts: Vec<String> = log
        .iter()
        .filter(|r| r.method == "PUT")
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(
        puts.first().map(String::as_str),
        Some(format!("{VERSION}/manifest.json").as_str()),
        "the claim lands first: {puts:?}"
    );
    // The claim's marker follows the claim bytes, still ahead of payload.
    assert_eq!(
        puts.get(1).map(String::as_str),
        Some(format!("{VERSION}/manifest.json.sha256").as_str()),
        "the claim marker precedes other names: {puts:?}"
    );
}

#[tokio::test]
async fn up_claim_first_refuses_a_name_outside_the_scan() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);

    let claim = ArtifactName::parse("ghost.json").unwrap();
    let err = nxr
        .up(src.path(), None, true, Some(claim), None)
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 2, "unknown claim name is misuse: {err}");
    let log = mock.requests();
    assert_eq!(log.iter().filter(|r| r.method == "PUT").count(), 0);
}
