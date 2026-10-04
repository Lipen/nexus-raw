//! Conformance: the v0.3 invariant matrix, driven through the core facade against the mock-nexus failure scenarios.

use std::path::Path;
use std::time::Duration;

use mock_nexus::{MockNexus, Outcome, ReqLog, Scenario};
use nexus_raw_core::sync::down::part_path;
use nexus_raw_core::{
    model::sibling, staging_dir, ArtifactName, Config, Digest, Enumeration, Error, Event, Manifest,
    Mode, Nxr, Summary,
};
use tempfile::TempDir;
use tokio::sync::mpsc;

const USER: &str = "ci";
const PASS: &str = "secret";
const VERSION: &str = "1.0.0";
const CONTENT: &[u8] = b"payload-0123456789";

fn config(mock: &MockNexus, auth: Option<String>) -> Config {
    config_at(dir_url(mock), auth)
}

/// `Config` at an explicit base URL.
fn config_at(base: String, auth: Option<String>) -> Config {
    Config {
        base,
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
    nxr.up(local.path(), None, true, None).await.unwrap();
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
    // freeze-upload holds the connection after the head: the attempt-level stall watchdog must surface a retryable transport failure instead of hanging on a socket nobody drains.
    let mock = MockNexus::start(Scenario::FreezeUpload).unwrap();
    let local = TempDir::new().unwrap();
    // Big enough that kernel socket buffers and the body channel fill: the write side must actually feel the freeze.
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
        nxr.up(local.path(), None, true, None),
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
    // The never-overwrite invariant, driven through the wire: a complete remote artifact with a different digest refuses the whole up and the remote bytes stay untouched.
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

    let err = nxr.up(local.path(), None, true, None).await.unwrap_err();
    assert_eq!(err.exit_code(), 1, "divergence is a data error, got {err}");
    assert_eq!(
        mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        remote,
        "the remote bytes must survive the refused up"
    );
}

#[tokio::test]
async fn sizeless_head_refuses_instead_of_overwriting() {
    // The proxy case: a 200 without Content-Length must not classify as Absent, or the digest comparison is skipped and the upload overwrites.
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

    let err = nxr.up(local.path(), None, true, None).await.unwrap_err();
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
    // DropConnection resets the first request per path.
    // Driven through the core client: the first request is reset on the wire, the retry succeeds, the artifact lands complete.
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
        .down(local.path(), Enumeration::Names(names(&["a.zip"])), false)
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
    // The part name is derived from the server-controlled object name, so a pre-placed symlink at the part path must fail the write (O_NOFOLLOW), never become a write gadget into the decoy.
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
        .down(local.path(), Enumeration::Names(vec![a]), false)
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
    // Same class as the part case, one stage later: bytes land fine, the local marker write must refuse to follow a symlink.
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
        .down(local.path(), Enumeration::Names(names(&["a.zip"])), false)
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
    // The up side of the rule: auto-generated markers go through the same NOFOLLOW open as the download side.
    // A symlink at the marker path fails the run before anything is sent.
    // The decoy stays intact.
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    std::fs::write(local.path().join("a.zip"), CONTENT).unwrap();
    let decoy = local.path().join("decoy.txt");
    std::fs::write(&decoy, b"decoy bytes").unwrap();
    std::os::unix::fs::symlink(&decoy, local.path().join("a.zip.sha256")).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let err = nxr.up(local.path(), None, true, None).await.unwrap_err();
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
    // A manifest is a "small" GET: past the 16 MiB cap the read refuses (exit 2) instead of slurping an unbounded body into memory.
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    mock.insert(
        &format!("{VERSION}/manifest.json"),
        &vec![0u8; 16 * 1024 * 1024 + 1],
    );
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
    // A directory without any .sha256 file still produces Complete remote objects (§5.2 markers-on-by-default).
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    std::fs::write(local.path().join("a.zip"), CONTENT).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    nxr.up(local.path(), None, true, None).await.unwrap();
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

    nxr.up(local.path(), None, false, None).await.unwrap();
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

    nxr.up(local.path(), None, true, None).await.unwrap();
    assert_eq!(mock.put_count(&format!("{VERSION}/a.zip")), 1);
    // The marker was accepted by the server but silently dropped.
    assert!(mock.store_get(&format!("{VERSION}/a.zip.sha256")).is_none());

    // The second up re-sends: the remote copy is not provably complete.
    let summary = nxr.up(local.path(), None, true, None).await.unwrap();
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

    nxr.up(local.path(), None, true, None).await.unwrap();
    let puts = mock.put_count(&format!("{VERSION}/a.zip"));
    let summary = nxr.up(local.path(), None, true, None).await.unwrap();
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

    let err = nxr.up(local.path(), None, true, None).await.unwrap_err();
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
        .up(local.path(), Some(manifest.names), true, None)
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

    nxr.up(local.path(), None, true, None).await.unwrap();
    assert_eq!(
        mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        CONTENT
    );
}

#[tokio::test]
async fn retry_events_name_the_object_they_retry() {
    // A retried marker must never arrive as an unnamed event: the console line for it reads `↻ : retry 2 (…)`, and an NDJSON consumer cannot tell which object stalled.
    // Markers and other small objects travel by URL, so their name comes from the URL when the caller has no ArtifactName to pass.
    let mock = MockNexus::start(Scenario::Flaky { first_failures: 1 }).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    nxr.up(local.path(), None, true, None).await.unwrap();

    let retries: Vec<(String, u32)> = collect_events(&mut rx)
        .into_iter()
        .filter_map(|e| match e {
            Event::Retrying { name, attempt, .. } => Some((name, attempt)),
            _ => None,
        })
        .collect();
    assert!(!retries.is_empty(), "flaky server produced no retry event");
    for (name, attempt) in &retries {
        assert!(!name.is_empty(), "nameless retry event, attempt {attempt}");
    }
    assert!(
        retries.iter().any(|(name, _)| name == "a.zip.sha256"),
        "the marker retry should name the marker: {retries:?}"
    );
}

#[tokio::test]
async fn empty_dir_refuses_up() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let err = nxr.up(local.path(), None, true, None).await.unwrap_err();
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
    nxr.up(src.path(), None, true, None).await.unwrap();

    let dst = TempDir::new().unwrap();
    let summary = nxr
        .down(dst.path(), Enumeration::Names(names(&["a.zip"])), true)
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
async fn duplicate_explicit_names_download_once() {
    // A repeated name is one artifact: two workers racing on one part file would
    // land a corrupt object behind a valid marker.
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    nxr.up(local.path(), None, true, None).await.unwrap();

    let dst = TempDir::new().unwrap();
    let summary = nxr
        .down(
            dst.path(),
            Enumeration::Names(names(&["a.zip", "a.zip"])),
            false,
        )
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1);
    assert!(summary.failed.is_empty());
    assert_eq!(std::fs::read(dst.path().join("a.zip")).unwrap(), CONTENT);
}

#[tokio::test]
async fn down_resumes_from_part_with_range() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    nxr.up(src.path(), None, true, None).await.unwrap();

    // Pre-seed the part file with the first bytes, as an interrupted run left it.
    let dst = TempDir::new().unwrap();
    let name = ArtifactName::parse("a.zip").unwrap();
    let part = part_path(dst.path(), &name);
    std::fs::write(&part, &CONTENT[..5]).unwrap();

    let summary = nxr
        .down(dst.path(), Enumeration::Names(names(&["a.zip"])), false)
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
    nxr.up(src.path(), None, true, None).await.unwrap();

    // The crash edge: the process died after the download finished but before the rename, so the part already holds the whole object.
    let dst = TempDir::new().unwrap();
    let name = ArtifactName::parse("a.zip").unwrap();
    let part = part_path(dst.path(), &name);
    std::fs::write(&part, CONTENT).unwrap();

    let summary = nxr
        .down(dst.path(), Enumeration::Names(names(&["a.zip"])), false)
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
    nxr.up(src.path(), None, true, None).await.unwrap();

    let manifest = nxr.manifest_at_base().await.unwrap().unwrap();
    assert_eq!(manifest.names.len(), 2);
    let dst = TempDir::new().unwrap();
    let summary = nxr
        .down(dst.path(), Enumeration::Manifest(manifest), false)
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
    nxr.up(src.path(), None, true, None).await.unwrap();

    let dst = TempDir::new().unwrap();
    let err = nxr
        .down(dst.path(), Enumeration::Names(names(&["a.zip"])), true)
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
        .down(dst.path(), Enumeration::Names(names(&["ghost.bin"])), true)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Missing { .. }), "got {err:?}");
}

#[tokio::test]
async fn down_markerless_remote_writes_computed_marker() {
    // Markerless remote: bytes without a marker.
    // Down fetches them and computes the marker locally (§5.2).
    // Without a sibling there is nothing to verify a resume against, so an existing part is ignored entirely (§5.1): the name downloads from zero, however stale the part is.
    let mock = MockNexus::start(Scenario::Markerless).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    nxr.up(src.path(), None, false, None).await.unwrap(); // --no-sha

    let dst = TempDir::new().unwrap();
    let name = ArtifactName::parse("a.zip").unwrap();
    let part = part_path(dst.path(), &name);
    std::fs::write(&part, b"stale-bytes-that-must-never-survive").unwrap();
    nxr.down(dst.path(), Enumeration::Names(names(&["a.zip"])), false)
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

// ---------------------------------------------------------------- mirror

/// The version document the fixtures pour.
const DOC: &[u8] = br#"{"schema_version":1,"version":"1.0.0","artifacts":["a.zip","b.bin"]}"#;

/// Seed a source repository through `up`: two payloads, the version document and the manifest enumerating them, every name with its marker.
/// The manifest lists the version document first, the convention the mirror's claim-first rule reads.
async fn seed_source(mock: &MockNexus) -> Nxr {
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    seed_complete(local.path(), "b.bin", b"second payload");
    seed_complete(local.path(), "version.json", DOC);
    std::fs::write(
        local.path().join("manifest.json"),
        br#"{"artifacts":["version.json","a.zip","b.bin","manifest.json"]}"#,
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(mock, None), tx).unwrap();
    nxr.up(local.path(), None, true, None).await.unwrap();
    nxr
}

/// Every object the fixture puts into a repository: bytes and markers, in manifest order.
const FIXTURE: &[&str] = &["version.json", "a.zip", "b.bin", "manifest.json"];

#[tokio::test]
async fn mirror_pours_byte_equal_trees_through_a_manifest() {
    // The base conformance scenario: atomic on both sides, `--manifest` as the enumeration.
    let src_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let dst_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let src = seed_source(&src_mock).await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let dst = Nxr::new(config(&dst_mock, None), tx).unwrap();

    let manifest = src.manifest_at_base().await.unwrap().unwrap();
    let summary = src
        .mirror(&dst, Enumeration::Manifest(manifest))
        .await
        .unwrap();
    assert_eq!(summary.uploaded, FIXTURE.len());
    assert_eq!(summary.skipped, 0);
    assert!(summary.failed.is_empty());

    // Byte-equal trees: every object and every marker.
    for name in FIXTURE {
        let path = format!("{VERSION}/{name}");
        assert_eq!(
            dst_mock.store_get(&path),
            src_mock.store_get(&path),
            "{name} bytes"
        );
        let marker = format!("{path}.sha256");
        assert_eq!(
            dst_mock.store_get(&marker),
            src_mock.store_get(&marker),
            "{name} marker"
        );
        assert!(
            dst_mock.store_get(&marker).is_some(),
            "{name} keeps a marker"
        );
    }

    // The enumeration led with the version document, so it claimed the run:
    // its bytes and its marker land before any other name's writes.
    let puts: Vec<String> = dst_mock
        .requests()
        .iter()
        .filter(|r| r.method == "PUT")
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(
        puts.first().map(String::as_str),
        Some(format!("{VERSION}/version.json").as_str()),
        "the version document is claimed first: {puts:?}"
    );
    assert_eq!(
        puts.get(1).map(String::as_str),
        Some(format!("{VERSION}/version.json.sha256").as_str()),
        "the claim marker precedes the other names: {puts:?}"
    );

    // A clean run consumes its staging dir.
    assert!(!staging_dir(src.base(), dst.base()).exists());
}

#[tokio::test]
async fn second_mirror_skips_with_zero_content_gets() {
    let src_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let dst_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let src = seed_source(&src_mock).await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let dst = Nxr::new(config(&dst_mock, None), tx).unwrap();

    // Explicit names keep the enumeration read out of the way: any GET to a bytes path
    // in the window below is a content fetch.
    let fixture_names = names(FIXTURE);
    src.mirror(&dst, Enumeration::Names(fixture_names.clone()))
        .await
        .unwrap();
    let puts_after_first = dst_mock.put_count(&format!("{VERSION}/a.zip"));

    // The rerun skips everything and fetches no content: the probes are a HEAD on the
    // bytes and a GET on the sibling.
    let after_first = src_mock.requests().len();
    let summary = src
        .mirror(&dst, Enumeration::Names(fixture_names))
        .await
        .unwrap();
    assert_eq!(summary.uploaded, 0);
    assert_eq!(summary.skipped, FIXTURE.len());
    let content_gets = src_mock.requests()[after_first..]
        .iter()
        .filter(|r| {
            r.method == "GET"
                && FIXTURE
                    .iter()
                    .any(|name| r.path == format!("{VERSION}/{name}"))
        })
        .count();
    assert_eq!(content_gets, 0, "no content GETs on a converged mirror");
    assert_eq!(
        dst_mock.put_count(&format!("{VERSION}/a.zip")),
        puts_after_first,
        "a converged mirror writes nothing"
    );
}

#[tokio::test]
async fn mirror_refuses_diverged_destination_untouched() {
    let src_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let dst_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let src = seed_source(&src_mock).await;
    // The destination holds a complete object under the same name with a different digest.
    let foreign = b"foreign bytes";
    dst_mock.insert(&format!("{VERSION}/a.zip"), foreign);
    dst_mock.insert(
        &format!("{VERSION}/a.zip.sha256"),
        sibling::format_line("a.zip", &Digest::of_bytes(foreign)).as_bytes(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let dst = Nxr::new(config(&dst_mock, None), tx).unwrap();

    let err = src
        .mirror(&dst, Enumeration::Names(names(&["a.zip"])))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Mismatch { .. }), "got {err:?}");
    assert_eq!(err.exit_code(), 1, "divergence is a data error");
    // The destination is untouched: no rewrite, bytes and marker as seeded.
    assert_eq!(
        dst_mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        foreign
    );
    assert_eq!(dst_mock.put_count(&format!("{VERSION}/a.zip")), 0);
    assert_eq!(dst_mock.put_count(&format!("{VERSION}/a.zip.sha256")), 0);
}

#[tokio::test]
async fn mirror_recovers_through_flaky() {
    // The first two requests per destination path answer 503; the default four attempts
    // absorb them, so one invocation pours the version and the repeat is a pure skip.
    let src_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let dst_mock = MockNexus::start(Scenario::Flaky { first_failures: 2 }).unwrap();
    let src = seed_source(&src_mock).await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let dst = Nxr::new(config(&dst_mock, None), tx).unwrap();

    let enum_names = || names(&["a.zip", "b.bin"]);
    let summary = src
        .mirror(&dst, Enumeration::Names(enum_names()))
        .await
        .unwrap();
    assert_eq!(summary.uploaded, 2);
    assert_eq!(
        dst_mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        CONTENT
    );
    assert_eq!(
        dst_mock.store_get(&format!("{VERSION}/b.bin")).unwrap(),
        b"second payload"
    );
    assert!(dst_mock
        .store_get(&format!("{VERSION}/a.zip.sha256"))
        .is_some());
    assert!(dst_mock
        .store_get(&format!("{VERSION}/b.bin.sha256"))
        .is_some());

    // The same command again: everything skipped, nothing rewritten.
    let summary = src
        .mirror(&dst, Enumeration::Names(enum_names()))
        .await
        .unwrap();
    assert_eq!(summary.uploaded, 0);
    assert_eq!(summary.skipped, 2);
    assert_eq!(dst_mock.put_count(&format!("{VERSION}/a.zip")), 1);
}

#[tokio::test]
async fn mirror_resumes_staged_part_with_range() {
    let src_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let dst_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let src = seed_source(&src_mock).await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let dst = Nxr::new(config(&dst_mock, None), tx).unwrap();

    // Pre-seed the staging part with the first bytes, as a killed run left it.
    let name = ArtifactName::parse("a.zip").unwrap();
    let staging = staging_dir(src.base(), dst.base());
    std::fs::create_dir_all(&staging).unwrap();
    let part = part_path(&staging, &name);
    std::fs::write(&part, &CONTENT[..5]).unwrap();

    let summary = src
        .mirror(&dst, Enumeration::Names(names(&["a.zip"])))
        .await
        .unwrap();
    assert_eq!(summary.uploaded, 1);
    // The source answered 206 Partial Content for the resumed object.
    let hit = src_mock.requests().iter().any(|r| {
        r.method == "GET"
            && r.path == format!("{VERSION}/a.zip")
            && matches!(r.outcome, Outcome::Status(206))
    });
    assert!(
        hit,
        "expected a 206 range response, log: {:?}",
        src_mock.requests()
    );
    assert_eq!(
        dst_mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        CONTENT
    );
    assert!(dst_mock
        .store_get(&format!("{VERSION}/a.zip.sha256"))
        .is_some());
}

#[tokio::test]
async fn mirror_completes_a_markerless_source() {
    // The Markerless scenario never stores markers: the source holds bare bytes.
    // The mirror stages them, computes the digest and writes the marker at the destination.
    let src_mock = MockNexus::start(Scenario::Markerless).unwrap();
    let dst_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let src = seed_source(&src_mock).await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let dst = Nxr::new(config(&dst_mock, None), tx).unwrap();

    assert!(src_mock
        .store_get(&format!("{VERSION}/a.zip.sha256"))
        .is_none());
    let summary = src
        .mirror(&dst, Enumeration::Names(names(&["a.zip"])))
        .await
        .unwrap();
    assert_eq!(summary.uploaded, 1);
    assert_eq!(
        dst_mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        CONTENT
    );
    assert_eq!(
        dst_mock
            .store_get(&format!("{VERSION}/a.zip.sha256"))
            .unwrap(),
        sibling::format_line("a.zip", &Digest::of_bytes(CONTENT)).into_bytes()
    );
}

#[tokio::test]
async fn mirror_skips_when_a_markerless_source_matches_a_complete_destination() {
    // No marker at the source: the staged digest is compared against the destination
    // marker. Equal: the destination is left alone, nothing is rewritten.
    let src_mock = MockNexus::start(Scenario::Markerless).unwrap();
    let dst_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let src = seed_source(&src_mock).await;
    dst_mock.insert(&format!("{VERSION}/a.zip"), CONTENT);
    dst_mock.insert(
        &format!("{VERSION}/a.zip.sha256"),
        sibling::format_line("a.zip", &Digest::of_bytes(CONTENT)).as_bytes(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let dst = Nxr::new(config(&dst_mock, None), tx).unwrap();

    let summary = src
        .mirror(&dst, Enumeration::Names(names(&["a.zip"])))
        .await
        .unwrap();
    assert_eq!(summary.uploaded, 0);
    assert_eq!(summary.skipped, 1);
    assert_eq!(dst_mock.put_count(&format!("{VERSION}/a.zip")), 0);
    assert_eq!(dst_mock.put_count(&format!("{VERSION}/a.zip.sha256")), 0);
}

#[tokio::test]
async fn mirror_missing_name_is_data_error() {
    let src_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let dst_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let src = seed_source(&src_mock).await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let dst = Nxr::new(config(&dst_mock, None), tx).unwrap();

    let err = src
        .mirror(&dst, Enumeration::Names(names(&["ghost.bin"])))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Missing { .. }), "got {err:?}");
    assert_eq!(err.exit_code(), 1);
}

#[tokio::test]
async fn mirror_uses_the_destination_client_for_its_writes() {
    // The source credentials must never reach the destination host, and the writes
    // must carry the destination credentials: the writes follow the destination.
    // The source runs open, the destination demands Basic auth, and only the
    // destination facade holds the credentials.
    let src_mock = MockNexus::start(Scenario::Atomic).unwrap();
    let dst_mock = MockNexus::start(Scenario::Auth401 {
        user: USER.to_owned(),
        pass: PASS.to_owned(),
    })
    .unwrap();
    let src = seed_source(&src_mock).await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let dst = Nxr::new(
        config(
            &dst_mock,
            Some(format!(
                "Basic {}",
                nexus_raw_core::creds::basic(USER, PASS)
            )),
        ),
        tx,
    )
    .unwrap();

    let manifest = src.manifest_at_base().await.unwrap().unwrap();
    let summary = src
        .mirror(&dst, Enumeration::Manifest(manifest))
        .await
        .unwrap();
    assert_eq!(summary.uploaded, FIXTURE.len());

    // The bytes landed behind the destination's own credentials.
    for name in FIXTURE {
        assert!(
            dst_mock.store_get(&format!("{VERSION}/{name}")).is_some(),
            "{name} reached the authenticated destination"
        );
    }
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
    let err = nxr.up(local.path(), None, true, None).await.unwrap_err();
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
    nxr.up(local.path(), None, true, None).await.unwrap();
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
    nxr.up(src.path(), None, true, None).await.unwrap();

    let dst = TempDir::new().unwrap();
    let err = nxr
        .down(dst.path(), Enumeration::Names(names(&["a.zip"])), true)
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
    nxr.up(local.path(), None, true, None).await.unwrap();
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
    nxr.up(src.path(), None, true, None).await.unwrap();
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
    nxr.up(src.path(), None, true, None).await.unwrap();

    // The part holds bytes of an object that no longer matches the sibling: an interrupted download of an older remote version.
    let dst = TempDir::new().unwrap();
    let name = ArtifactName::parse("a.zip").unwrap();
    let part = part_path(dst.path(), &name);
    std::fs::write(&part, b"stale-prefix-of-an-old-object").unwrap();

    let summary = nxr
        .down(dst.path(), Enumeration::Names(names(&["a.zip"])), false)
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
    let _summary = nxr.up(src.path(), None, true, Some(claim)).await.unwrap();

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
        .up(src.path(), None, true, Some(claim))
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 2, "unknown claim name is misuse: {err}");
    let log = mock.requests();
    assert_eq!(log.iter().filter(|r| r.method == "PUT").count(), 0);
}

// ---------------------------------------------------------------- rm

/// A version published for the rm tests: two artifacts plus the version document, manifest at the base.
async fn publish_version(nxr: &Nxr, mock: &MockNexus, local: &TempDir) {
    seed_complete(local.path(), "a.zip", CONTENT);
    seed_complete(local.path(), "b.bin", b"other bytes");
    nxr.up(local.path(), None, true, None).await.unwrap();
    mock.insert(
        &format!("{VERSION}/version.json"),
        br#"{"version":"1.0.0","artifacts":["a.zip","b.bin"]}"#,
    );
    mock.insert(
        &format!("{VERSION}/manifest.json"),
        br#"{"artifacts":["a.zip","b.bin","version.json"]}"#,
    );
}

/// rm deletes a whole version: every enumerated name with its marker, the version document included, the enumeration manifest excluded.
#[tokio::test]
async fn rm_removes_a_whole_version() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    publish_version(&nxr, &mock, &local).await;

    let manifest = nxr.manifest_at_base().await.unwrap().unwrap();
    let summary = nxr.rm(Enumeration::Manifest(manifest)).await.unwrap();
    assert_eq!(summary.removed, 3);
    assert_eq!(summary.skipped, 0);
    assert!(summary.failed.is_empty());

    // Everything enumerated is gone, down to the markers; the manifest survives as the enumeration source of a rerun.
    for name in [
        "a.zip",
        "a.zip.sha256",
        "b.bin",
        "b.bin.sha256",
        "version.json",
    ] {
        assert!(
            mock.store_get(&format!("{VERSION}/{name}")).is_none(),
            "{name} must be gone"
        );
    }
    assert!(mock
        .store_get(&format!("{VERSION}/manifest.json"))
        .is_some());

    // The wire order is the reverse of publishing: the marker DELETE precedes the bytes DELETE of the same name.
    let log = mock.requests();
    for name in ["a.zip", "b.bin"] {
        let marker_at = log
            .iter()
            .position(|r| r.method == "DELETE" && r.path == format!("{VERSION}/{name}.sha256"))
            .unwrap();
        let bytes_at = log
            .iter()
            .position(|r| r.method == "DELETE" && r.path == format!("{VERSION}/{name}"))
            .unwrap();
        assert!(
            marker_at < bytes_at,
            "the marker of {name} must be deleted first"
        );
    }

    let events = collect_events(&mut rx);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Removing { .. }))
            .count(),
        3,
        "one removing event per name: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Removed { .. }))
            .count(),
        3
    );
    assert!(matches!(events.last(), Some(Event::Summary(_))));
    drop(rx);
}

/// Deletion is idempotent: the second rm sees only 404s and still succeeds.
#[tokio::test]
async fn second_rm_is_all_404_and_exits_clean() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    publish_version(&nxr, &mock, &local).await;

    let manifest = nxr.manifest_at_base().await.unwrap().unwrap();
    nxr.rm(Enumeration::Manifest(manifest)).await.unwrap();

    let manifest = nxr.manifest_at_base().await.unwrap().unwrap();
    let requests_before = mock.requests().len();
    let summary = nxr.rm(Enumeration::Manifest(manifest)).await.unwrap();
    assert_eq!(summary.removed, 0);
    assert_eq!(summary.skipped, 3, "every name reports missing");

    let deletes: Vec<Outcome> = mock.requests()[requests_before..]
        .iter()
        .filter(|r| r.method == "DELETE")
        .map(|r| r.outcome.clone())
        .collect();
    assert_eq!(deletes.len(), 6, "marker + bytes per name");
    assert!(
        deletes.iter().all(|o| matches!(o, Outcome::Status(404))),
        "the rerun must see only 404s: {deletes:?}"
    );
}

/// The read-only repository refuses the deletion: exit 1, and nothing ever left the store.
#[tokio::test]
async fn readonly_refuses_rm_and_changes_nothing() {
    let mock = MockNexus::start(Scenario::ReadOnly).unwrap();
    let local = TempDir::new().unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    publish_version(&nxr, &mock, &local).await;

    let manifest = nxr.manifest_at_base().await.unwrap().unwrap();
    let err = nxr.rm(Enumeration::Manifest(manifest)).await.unwrap_err();
    assert_eq!(
        err.exit_code(),
        1,
        "read-only refusal is a data verdict: {err}"
    );
    assert!(
        err.to_string().contains("read-only"),
        "the error names the refusal: {err}"
    );
    assert!(err.hint().is_some());

    // The refusal fired on the first request, so the store still holds everything.
    for name in [
        "a.zip",
        "a.zip.sha256",
        "b.bin",
        "b.bin.sha256",
        "version.json",
    ] {
        assert!(
            mock.store_get(&format!("{VERSION}/{name}")).is_some(),
            "{name} must survive a refused rm"
        );
    }
    assert!(
        !mock
            .requests()
            .iter()
            .any(|r| r.method == "DELETE" && matches!(r.outcome, Outcome::Status(200..=300))),
        "no DELETE may succeed against a read-only repository"
    );
}

/// `rm --dry-run` probes and plans, but the mock never sees a DELETE and the bytes never move.
#[tokio::test]
async fn rm_dry_run_plans_and_touches_nothing() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    publish_version(&nxr, &mock, &local).await;
    mock.insert(
        &format!("{VERSION}/manifest.json"),
        br#"{"artifacts":["a.zip","b.bin","version.json","ghost.bin"]}"#,
    );

    let manifest = nxr.manifest_at_base().await.unwrap().unwrap();
    let plan = nxr.rm_plan(Enumeration::Manifest(manifest)).await.unwrap();
    let planned: Vec<String> = plan
        .iter()
        .map(|a| match a {
            nexus_raw_core::RmAction::Remove { name, .. } => format!("rm {name}"),
            nexus_raw_core::RmAction::Missing { name } => format!("missing {name}"),
        })
        .collect();
    assert_eq!(
        planned,
        [
            "rm a.zip",
            "rm b.bin",
            "rm version.json",
            "missing ghost.bin"
        ],
        "the plan separates present names from absent ones: {planned:?}"
    );

    assert!(
        !mock.requests().iter().any(|r| r.method == "DELETE"),
        "a dry run must not DELETE anything"
    );
    // Probes are reads; the store itself is untouched.
    assert!(mock.store_get(&format!("{VERSION}/a.zip")).is_some());
    assert!(mock.store_get(&format!("{VERSION}/ghost.bin")).is_none());
}

/// An enumeration that produces no names refuses, exactly like `down`.
#[tokio::test]
async fn rm_without_names_refuses_with_enumeration_error() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let err = nxr.rm(Enumeration::Names(vec![])).await.unwrap_err();
    assert!(matches!(err, Error::Enumerate { .. }), "{err}");
    assert_eq!(err.exit_code(), 1);
}

/// `point --clear` deletes the pointer, and the rerun is a normal `absent`.
#[tokio::test]
async fn point_clear_is_idempotent() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let url = format!("{}latest", mock.base_url());

    nxr.channel_set(&url, "1.0.0", false).await.unwrap();
    assert!(mock.store_get("latest").is_some());

    use nexus_raw_core::ClearOutcome;
    assert_eq!(nxr.point_clear(&url).await.unwrap(), ClearOutcome::Cleared);
    assert!(mock.store_get("latest").is_none());
    assert_eq!(nxr.point_clear(&url).await.unwrap(), ClearOutcome::Absent);
}

/// A read-only repository refuses the pointer deletion and keeps the file.
#[tokio::test]
async fn point_clear_on_readonly_refuses() {
    let mock = MockNexus::start(Scenario::ReadOnly).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    mock.insert("latest", b"1.0.0\n");

    let url = format!("{}latest", mock.base_url());
    let err = nxr.point_clear(&url).await.unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert!(
        mock.store_get("latest").is_some(),
        "the pointer must survive"
    );
}

// ---------------------------------------------------------------- group

/// A repository-shaped base: the `/repository/<name>/` prefix that a real group and its members carry.
/// The group forwards request paths verbatim, so members must hold the objects under the same shape.
fn repo_url(mock: &MockNexus) -> String {
    format!("{}repository/raw/{VERSION}/", mock.base_url())
}

/// Publish one complete artifact into a member's own store under the group path shape.
async fn seed_member(mock: &MockNexus, name: &str, content: &[u8]) {
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), name, content);
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config_at(repo_url(mock), None), tx).unwrap();
    nxr.up(src.path(), None, true, None).await.unwrap();
}

/// The group forwards reads to the members in order and relays the first `2xx`: the artifact lives only in the second member.
/// The walk is visible in the member logs: the first member answers the forwarded probe with a 404, the second serves the object.
#[tokio::test]
async fn down_through_group_serves_the_first_member_holding_the_object() {
    let first = MockNexus::start(Scenario::Atomic).unwrap();
    let second = MockNexus::start(Scenario::Atomic).unwrap();
    seed_member(&second, "b.bin", b"second member bytes").await;
    let group = MockNexus::start_group(&[&first, &second]).unwrap();

    let dst = TempDir::new().unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config_at(repo_url(&group), None), tx).unwrap();
    let summary = nxr
        .down(dst.path(), Enumeration::Names(names(&["b.bin"])), true)
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1, "{summary:?}");
    assert_eq!(
        std::fs::read(dst.path().join("b.bin")).unwrap(),
        b"second member bytes"
    );
    let marker = std::fs::read_to_string(dst.path().join("b.bin.sha256")).unwrap();
    assert_eq!(
        marker,
        sibling::format_line("b.bin", &Digest::of_bytes(b"second member bytes"))
    );

    // Member order: the first member was probed and missed, the second served.
    let missed = first
        .requests()
        .iter()
        .any(|r| r.method == "GET" && matches!(r.outcome, Outcome::Status(404)));
    let served = second
        .requests()
        .iter()
        .any(|r| r.method == "GET" && matches!(r.outcome, Outcome::Status(200)));
    assert!(
        missed,
        "the walk must probe the first member: {:?}",
        first.requests()
    );
    assert!(
        served,
        "the second member must serve the object: {:?}",
        second.requests()
    );
}

/// A failing member is skipped inside the group walk: a flaky member's 503 never reaches the client, so the retry loop never fires.
/// One client request per object, and both members were hit.
#[tokio::test]
async fn down_through_group_skips_a_failing_member_without_a_client_retry() {
    let flaky = MockNexus::start(Scenario::Flaky {
        first_failures: 999,
    })
    .unwrap();
    let atomic = MockNexus::start(Scenario::Atomic).unwrap();
    seed_member(&atomic, "b.bin", CONTENT).await;
    let group = MockNexus::start_group(&[&flaky, &atomic]).unwrap();

    let dst = TempDir::new().unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config_at(repo_url(&group), None), tx).unwrap();
    let summary = nxr
        .down(dst.path(), Enumeration::Names(names(&["b.bin"])), true)
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1, "{summary:?}");
    assert_eq!(std::fs::read(dst.path().join("b.bin")).unwrap(), CONTENT);

    // Exactly one client request for the object: had the 503 surfaced, the client would have retried and the log would repeat.
    let log = group.requests();
    let object_gets: Vec<&ReqLog> = log
        .iter()
        .filter(|r| r.method == "GET" && r.path == format!("repository/raw/{VERSION}/b.bin"))
        .collect();
    assert_eq!(
        object_gets.len(),
        1,
        "no client retry may happen: {:?}",
        log
    );
    assert_eq!(object_gets[0].outcome, Outcome::Status(200));
    // The walk reached both members: flaky refused, atomic served.
    assert!(
        flaky
            .requests()
            .iter()
            .any(|r| matches!(r.outcome, Outcome::Status(503))),
        "{:?}",
        flaky.requests()
    );
    assert!(atomic
        .requests()
        .iter()
        .any(|r| r.method == "GET" && matches!(r.outcome, Outcome::Status(200))));
}

/// The group refuses uploads before any member is contacted: the PUT answers 405.
/// The client maps the 405 to `Error::Http`, whose verdict is exit 3; only 5xx are retryable, so the refusal fails fast.
#[tokio::test]
async fn up_against_a_group_refuses_with_405() {
    let first = MockNexus::start(Scenario::Atomic).unwrap();
    let second = MockNexus::start(Scenario::Atomic).unwrap();
    let group = MockNexus::start_group(&[&first, &second]).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config_at(repo_url(&group), None), tx).unwrap();

    let err = nxr.up(local.path(), None, true, None).await.unwrap_err();
    assert!(
        matches!(&err, Error::Http { status: 405, .. }),
        "got {err:?}"
    );
    assert_eq!(
        err.exit_code(),
        3,
        "a refused PUT is the HTTP verdict: {err}"
    );
    assert!(
        err.to_string().contains("405"),
        "the status stays visible: {err}"
    );

    // The refusal is the group's own: no member saw a write, and nothing landed anywhere.
    let path = format!("repository/raw/{VERSION}/a.zip");
    assert_eq!(group.put_count(&path), 0);
    assert!(group
        .requests()
        .iter()
        .any(|r| r.method == "PUT" && matches!(r.outcome, Outcome::Status(405))));
    for member in [&first, &second] {
        assert!(
            member
                .requests()
                .iter()
                .all(|r| r.method != "PUT" && r.method != "DELETE"),
            "a group write must not reach a member: {:?}",
            member.requests()
        );
        assert!(member.store_get(&path).is_none());
    }
}

/// A group refuses the deletion: the 405 rides the existing read-only mapping, which is a data verdict (exit 1).
/// Every member keeps its bytes, and no member ever sees a DELETE.
#[tokio::test]
async fn rm_against_a_group_refuses_as_read_only() {
    let first = MockNexus::start(Scenario::Atomic).unwrap();
    let second = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    let (tx, _rx) = mpsc::unbounded_channel();
    let member = Nxr::new(config_at(repo_url(&second), None), tx).unwrap();
    member.up(local.path(), None, true, None).await.unwrap();
    second.insert(
        &format!("repository/raw/{VERSION}/manifest.json"),
        br#"{"artifacts":["a.zip"]}"#,
    );
    let group = MockNexus::start_group(&[&first, &second]).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config_at(repo_url(&group), None), tx).unwrap();

    let manifest = nxr.manifest_at_base().await.unwrap().unwrap();
    let err = nxr.rm(Enumeration::Manifest(manifest)).await.unwrap_err();
    assert!(
        matches!(&err, Error::ReadOnly { status: 405, .. }),
        "got {err:?}"
    );
    assert_eq!(
        err.exit_code(),
        1,
        "the read-only refusal is a data verdict: {err}"
    );
    assert!(err.to_string().contains("read-only"), "{err}");

    // Nothing was deleted anywhere.
    assert!(second
        .store_get(&format!("repository/raw/{VERSION}/a.zip"))
        .is_some());
    assert!(second
        .store_get(&format!("repository/raw/{VERSION}/a.zip.sha256"))
        .is_some());
    for member in [&first, &second] {
        assert!(
            !member.requests().iter().any(|r| r.method == "DELETE"),
            "no DELETE may reach a member: {:?}",
            member.requests()
        );
    }
}

// ---------------------------------------------------------------- wave 1: channel repair, marker validation

#[tokio::test]
async fn channel_set_repairs_a_garbage_file() {
    // The set command is the tool that repairs a broken channel: garbage on the
    // current file must not block the write, with or without --if-forward.
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    let garbage = local.path().join("garbage.bin");
    std::fs::write(&garbage, b"not a token\nsecond line\n").unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    let channel_url = format!("{}latest", mock.base_url());
    nxr.put(&channel_url, &garbage, false).await.unwrap();

    // Reading the garbage still reports a data problem for `channel get`.
    let (tx2, _rx2) = mpsc::unbounded_channel();
    let reader = Nxr::new(config(&mock, None), tx2).unwrap();
    assert!(reader.channel_get(&channel_url).await.is_err());

    // But the write goes through and repairs the channel, forward guard included.
    let outcome = nxr.channel_set(&channel_url, "1.2.3", true).await.unwrap();
    assert!(matches!(
        outcome,
        nexus_raw_core::ChannelOutcome::Written { .. }
    ));
    assert_eq!(
        nxr.channel_get(&channel_url).await.unwrap().as_deref(),
        Some("1.2.3")
    );
}

#[tokio::test]
async fn put_sha_refuses_a_url_whose_marker_cannot_parse() {
    // A marker built from an unparseable segment would land a permanently Broken
    // object behind exit 0: the URL is refused before anything is written.
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let local = TempDir::new().unwrap();
    let file = local.path().join("f.txt");
    std::fs::write(&file, CONTENT).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let trailing = format!("{}dir/", mock.base_url());
    let err = nxr.put(&trailing, &file, true).await.unwrap_err();
    assert_eq!(err.exit_code(), 2, "got {err:?}");
    assert!(
        mock.store_get("dir/.sha256").is_none(),
        "no marker may land for an unparseable name"
    );

    // A plain URL still writes bytes and a marker as before.
    let plain = format!("{}f.txt", mock.base_url());
    let (size, digest) = nxr.put(&plain, &file, true).await.unwrap();
    assert_eq!(size, CONTENT.len() as u64);
    assert!(digest.is_some());
    assert!(mock.store_get("f.txt.sha256").is_some());
}

// ---------------------------------------------------------------- wave 2 scenarios: rate limit, auth 403, redirect, cut body

#[tokio::test]
async fn rate_limited_up_recovers_in_one_invocation() {
    // The first request per path answers 429 with a Retry-After header: the client honors the pause, spends an attempt and still lands the whole version.
    let mock = MockNexus::start(Scenario::RateLimit {
        first_429s: 1,
        retry_after_secs: 1,
    })
    .unwrap();
    let local = TempDir::new().unwrap();
    seed_complete(local.path(), "a.zip", CONTENT);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let summary = nxr.up(local.path(), None, true, None).await.unwrap();
    assert_eq!(summary.uploaded, 1);
    assert_eq!(summary.downloaded, 0);
    assert!(summary.failed.is_empty());
    assert_eq!(
        mock.store_get(&format!("{VERSION}/a.zip")).unwrap(),
        CONTENT
    );
    assert!(mock.store_get(&format!("{VERSION}/a.zip.sha256")).is_some());

    // Both throttled requests (the probe and the marker PUT) came back as named retry events.
    let retries: Vec<(String, u32)> = collect_events(&mut rx)
        .into_iter()
        .filter_map(|e| match e {
            Event::Retrying { name, attempt, .. } => Some((name, attempt)),
            _ => None,
        })
        .collect();
    assert!(
        !retries.is_empty(),
        "a rate-limited run must retry: {retries:?}"
    );
    assert!(retries.iter().all(|(name, _)| !name.is_empty()));
}

#[tokio::test]
async fn auth_403_surfaces_as_the_auth_error() {
    // auth-403 answers 403 to every unauthenticated request: the client maps it onto the auth error (exit 3), like a 401.
    let mock = MockNexus::start(Scenario::Auth403 {
        user: USER.to_owned(),
        pass: PASS.to_owned(),
    })
    .unwrap();
    mock.insert(&format!("{VERSION}/a.zip"), CONTENT);
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let dst = TempDir::new().unwrap();
    let err = nxr
        .down(dst.path(), Enumeration::Names(names(&["a.zip"])), true)
        .await
        .unwrap_err();
    match &err {
        Error::Auth { reason, .. } => assert!(reason.contains("403"), "got {err:?}"),
        other => panic!("expected the auth error, got {other:?}"),
    }
    assert_eq!(err.exit_code(), 3);
    assert!(!dst.path().join("a.zip").exists());
}

#[tokio::test]
async fn redirect_refuses_reads_with_http_301() {
    // Redirects are off (Policy::none): a 301 is never followed and surfaces as the plain HTTP error (exit 3).
    let mock = MockNexus::start(Scenario::Redirect {
        location_path: "/repository/raw/moved".to_owned(),
    })
    .unwrap();
    mock.insert(&format!("{VERSION}/a.zip"), CONTENT);
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();

    let dst = TempDir::new().unwrap();
    let err = nxr
        .down(dst.path(), Enumeration::Names(names(&["a.zip"])), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::Http { status: 301, .. } if err.exit_code() == 3),
        "expected http 301, got {err:?}"
    );
    assert!(!dst.path().join("a.zip").exists());
}

#[tokio::test]
async fn cut_body_down_completes_through_the_part() {
    // The first GET breaks mid-body with honest headers: the retry resumes from the part with Range and finishes the download.
    let mock = MockNexus::start(Scenario::CutBody {
        after_bytes: 8,
        fake_length: false,
    })
    .unwrap();
    // Seed the remote through the same mock: writes are never cut.
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    nxr.up(src.path(), None, true, None).await.unwrap();

    let dst = TempDir::new().unwrap();
    let summary = nxr
        .down(dst.path(), Enumeration::Names(names(&["a.zip"])), false)
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1);
    assert_eq!(std::fs::read(dst.path().join("a.zip")).unwrap(), CONTENT);

    // The retry hit the wire as a Range resume and the mock served the suffix as 206.
    assert!(
        mock.requests().iter().any(|r| r.method == "GET"
            && r.path == format!("{VERSION}/a.zip")
            && r.outcome == Outcome::Status(206)),
        "the retry must resume from the part: {:?}",
        mock.requests()
    );
}

#[tokio::test]
async fn fake_length_down_recovers_through_a_short_read() {
    // A lying Content-Length (1024 high) does not stall the client: the body ends before the declared total, the attempt fails as a transport short read and the retry resumes from the part.
    // The observed mechanism (pinned here) is the short-read retry, not a stall: headers arrive, bytes stop early.
    let mock = MockNexus::start(Scenario::CutBody {
        after_bytes: 8,
        fake_length: true,
    })
    .unwrap();
    let src = TempDir::new().unwrap();
    seed_complete(src.path(), "a.zip", CONTENT);
    let (tx, _rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&mock, None), tx).unwrap();
    nxr.up(src.path(), None, true, None).await.unwrap();

    let dst = TempDir::new().unwrap();
    let summary = nxr
        .down(dst.path(), Enumeration::Names(names(&["a.zip"])), false)
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1);
    // The final file holds exactly the real bytes, not the lied-about length.
    assert_eq!(std::fs::read(dst.path().join("a.zip")).unwrap(), CONTENT);
    assert!(
        mock.requests().iter().any(|r| r.method == "GET"
            && r.path == format!("{VERSION}/a.zip")
            && r.outcome == Outcome::Status(206)),
        "the retry must resume from the part: {:?}",
        mock.requests()
    );
}
