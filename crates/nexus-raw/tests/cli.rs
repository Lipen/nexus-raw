//! CLI conformance: the v0.3 command surface driven through the `nxr`
//! binary against `mock-nexus`.
//!
//! Covers: up/down happy path and skip detection, marker generation
//! (default, `--no-sha`), enumeration sources (`--name`, `--manifest`,
//! none), Range resume, the put/get/sha/head primitives, channel refs,
//! offline verify, auth gating, NDJSON events and the exit-code matrix.

use std::path::Path;
use std::process::{Command, Output};

use mock_nexus::{MockNexus, Outcome, ReqLog, Scenario};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

const NXR: &str = env!("CARGO_BIN_EXE_nxr");

// ---- fixtures -------------------------------------------------------------

/// Two distinct artifact payloads shared by most tests.
const ALPHA: &[u8] = b"alpha payload: the quick brown fox\n";
const BETA: &[u8] = b"beta payload: 0123456789\n";

fn server(scenario: Scenario) -> MockNexus {
    MockNexus::start(scenario).expect("mock nexus starts")
}

/// The version-directory URL every transfer test works on.
fn dir_url(srv: &MockNexus) -> String {
    format!("{}1.14.0/", srv.base_url())
}

fn root_url(srv: &MockNexus) -> String {
    srv.base_url()
}

/// Run `nxr` with a hermetic environment: ambient NXR_* credentials must
/// never leak into a test; pass `-u` explicitly instead.
fn nxr(args: &[&str]) -> Output {
    Command::new(NXR)
        .args(args)
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD")
        .output()
        .expect("nxr binary runs")
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Assert the exit code, dumping both streams on mismatch.
fn expect_exit(out: &Output, want: i32, what: &str) {
    assert_eq!(
        code(out),
        want,
        "{what}: exit mismatch\nstdout:\n{}\nstderr:\n{}",
        stdout(out),
        stderr(out)
    );
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("fixture write");
}

fn read_file(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(name)).expect("fixture read")
}

/// Lowercase hex sha256 of `bytes`: the digest oracle for markers.
fn hex_digest(bytes: &[u8]) -> String {
    let sum = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for b in sum {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// The canonical sha-sibling line: `<hex>  <name>\n`.
fn marker_line(name: &str, bytes: &[u8]) -> String {
    format!("{}  {name}\n", hex_digest(bytes))
}

/// Every PUT the mock has served so far.
fn put_requests(srv: &MockNexus) -> Vec<ReqLog> {
    srv.requests()
        .into_iter()
        .filter(|r| r.method == "PUT")
        .collect()
}

/// Parse every stdout line as a JSON object; nothing else may be printed.
fn ndjson(out: &Output) -> Vec<serde_json::Value> {
    stdout(out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("line {l:?} is not JSON: {e}")))
        .collect()
}

// ---- transfers ------------------------------------------------------------

/// 1. up then down round trip against Atomic: bytes and markers on the
///    server, bytes and markers on disk, exit 0 everywhere.
#[test]
fn up_down_roundtrip_atomic() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "b.bin", BETA);
    let base = dir_url(&srv);

    let up = nxr(&["up", src.path().to_str().unwrap(), &base]);
    expect_exit(&up, 0, "up happy path");
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
    assert_eq!(srv.store_get("1.14.0/b.bin").as_deref(), Some(BETA));
    assert_eq!(
        srv.store_get("1.14.0/a.zip.sha256").as_deref(),
        Some(marker_line("a.zip", ALPHA).as_bytes()),
    );
    assert_eq!(
        srv.store_get("1.14.0/b.bin.sha256").as_deref(),
        Some(marker_line("b.bin", BETA).as_bytes()),
    );

    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "down",
        &base,
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
        "--name",
        "b.bin",
    ]);
    expect_exit(&down, 0, "down happy path");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
    assert_eq!(read_file(dst.path(), "b.bin"), BETA);
    // down fetches the marker too: the local copy is sha256sum-complete.
    assert_eq!(
        read_file(dst.path(), "a.zip.sha256"),
        marker_line("a.zip", ALPHA).into_bytes()
    );
}

/// 2. up generates sha-siblings: the local dir starts without any marker,
///    yet the server ends up with one (and the local dir gains it).
#[test]
fn up_generates_markers_by_default() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    assert!(!src.path().join("a.zip.sha256").exists());

    let up = nxr(&["up", src.path().to_str().unwrap(), &dir_url(&srv)]);
    expect_exit(&up, 0, "up with markerless dir");
    assert_eq!(
        srv.store_get("1.14.0/a.zip.sha256").as_deref(),
        Some(marker_line("a.zip", ALPHA).as_bytes()),
        "the core must generate and upload the marker itself"
    );
    assert_eq!(
        read_file(src.path(), "a.zip.sha256"),
        marker_line("a.zip", ALPHA).into_bytes(),
        "the generated marker lands next to the bytes"
    );
}

/// 3. up --no-sha: bytes go up, no marker is generated or stored.
#[test]
fn up_no_sha_skips_markers() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);

    let up = nxr(&[
        "up",
        "--no-sha",
        src.path().to_str().unwrap(),
        &dir_url(&srv),
    ]);
    expect_exit(&up, 0, "up --no-sha");
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
    assert!(
        srv.store_get("1.14.0/a.zip.sha256").is_none(),
        "--no-sha must not store a marker"
    );
    assert!(!src.path().join("a.zip.sha256").exists());
}

/// 4. the second up skips everything: no new PUTs, summary all-skipped.
#[test]
fn second_up_is_a_pure_skip() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "b.bin", BETA);
    let base = dir_url(&srv);

    expect_exit(
        &nxr(&["up", src.path().to_str().unwrap(), &base]),
        0,
        "first up",
    );
    let puts_after_first = put_requests(&srv).len();

    let second = nxr(&["up", src.path().to_str().unwrap(), &base]);
    expect_exit(&second, 0, "second up");
    assert_eq!(
        put_requests(&srv).len(),
        puts_after_first,
        "a fully-current up must not PUT anything"
    );
    assert!(
        stdout(&second).contains("skipped 2"),
        "both artifacts must be reported skipped, got: {}",
        stdout(&second)
    );
}

/// 16. up --dry-run prints the plan and transfers nothing.
#[test]
fn dry_run_prints_plan_without_uploading() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);

    let dry = nxr(&[
        "up",
        "--dry-run",
        src.path().to_str().unwrap(),
        &dir_url(&srv),
    ]);
    expect_exit(&dry, 0, "up --dry-run");
    let out = stdout(&dry);
    assert!(out.contains("upload a.zip"), "plan line missing: {out}");
    assert!(put_requests(&srv).is_empty(), "dry-run must not PUT");
}

// ---- enumeration ----------------------------------------------------------

/// 5. down with no enumeration flags and no server manifest: exit 1 with
///    a hint about enumeration.
#[test]
fn down_without_enumeration_needs_a_source() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", ALPHA);
    let dst = TempDir::new().unwrap();

    let down = nxr(&["down", &dir_url(&srv), dst.path().to_str().unwrap()]);
    expect_exit(&down, 1, "down without enumeration source");
    let err = stderr(&down);
    assert!(err.contains("hint:"), "a hint line is required: {err}");
    assert!(
        err.contains("enumerat"),
        "the error names enumeration: {err}"
    );
    assert!(!dst.path().join("a.zip").exists());
}

/// 6. down --name fetches exactly the named artifact.
#[test]
fn down_explicit_name() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);
    expect_exit(&nxr(&["up", src.path().to_str().unwrap(), &base]), 0, "up");

    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "down",
        &base,
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
    ]);
    expect_exit(&down, 0, "down --name");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
}

/// 7. down --manifest with a local manifest file.
#[test]
fn down_manifest_from_local_file() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);
    expect_exit(&nxr(&["up", src.path().to_str().unwrap(), &base]), 0, "up");

    let manifest = TempDir::new().unwrap();
    let manifest_path = manifest.path().join("manifest.json");
    std::fs::write(&manifest_path, r#"{"artifacts":["a.zip"]}"#).unwrap();

    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "down",
        &base,
        dst.path().to_str().unwrap(),
        "--manifest",
        manifest_path.to_str().unwrap(),
    ]);
    expect_exit(&down, 0, "down --manifest <file>");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
}

/// 8. resume: a pre-seeded part file continues through a Range request;
///    the mock answers 206 and the assembled file is byte-perfect.
#[test]
fn get_resumes_from_part_with_range() {
    let srv = server(Scenario::Atomic);
    let content: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    srv.insert("big.bin", &content);
    let url = format!("{}big.bin", root_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("big.bin");
    let part = out.path().join("big.bin.part");
    let prefix = &content[..100_000];
    std::fs::write(&part, prefix).unwrap();

    let got = nxr(&[
        "get",
        "--continue",
        &url,
        "-o",
        target.to_str().unwrap(),
        "--retry",
        "1",
    ]);
    expect_exit(&got, 0, "resumed get");
    assert!(
        stdout(&got).contains("resumed from 100000"),
        "the resume must be reported: {}",
        stdout(&got)
    );
    assert_eq!(std::fs::read(&target).unwrap(), content, "byte-perfect");
    assert!(!part.exists(), "the part file is renamed away");
    // The Range header went out and was honored with 206.
    assert!(
        srv.requests()
            .iter()
            .any(|r| r.method == "GET" && r.path == "big.bin" && r.outcome == Outcome::Status(206)),
        "resume must send Range and get a 206: {:?}",
        srv.requests()
    );
}

// ---- primitives -----------------------------------------------------------

/// 9. put → get roundtrip, put --sha stores the sibling, sha prints the
///    digest of the remote bytes.
#[test]
fn put_get_roundtrip_with_sha_sibling() {
    let srv = server(Scenario::Atomic);
    let file = TempDir::new().unwrap();
    write_file(file.path(), "blob.bin", ALPHA);
    let file_path = file.path().join("blob.bin");
    let root = root_url(&srv);

    let plain_url = format!("{root}pub/plain.bin");
    let put = nxr(&["put", &plain_url, "-f", file_path.to_str().unwrap()]);
    expect_exit(&put, 0, "plain put");
    assert_eq!(srv.store_get("pub/plain.bin").as_deref(), Some(ALPHA));
    assert!(srv.store_get("pub/plain.bin.sha256").is_none());

    let got = TempDir::new().unwrap();
    let out_path = got.path().join("roundtrip.bin");
    let get = nxr(&["get", &plain_url, "-o", out_path.to_str().unwrap()]);
    expect_exit(&get, 0, "get after put");
    assert_eq!(std::fs::read(&out_path).unwrap(), ALPHA);

    let marked_url = format!("{root}pub/marked.bin");
    let put_sha = nxr(&[
        "put",
        "--sha",
        &marked_url,
        "-f",
        file_path.to_str().unwrap(),
    ]);
    expect_exit(&put_sha, 0, "put --sha");
    assert_eq!(
        srv.store_get("pub/marked.bin.sha256").as_deref(),
        Some(marker_line("marked.bin", ALPHA).as_bytes()),
    );

    let sha = nxr(&["sha", &marked_url]);
    expect_exit(&sha, 0, "sha of remote object");
    assert_eq!(stdout(&sha).trim(), hex_digest(ALPHA));
    let hex = stdout(&sha).trim().to_owned();
    assert_eq!(hex.len(), 64, "64 hex chars");
    assert!(
        hex.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "lowercase hex only: {hex}"
    );
}

/// 10. head reports the status; JSON mode carries status and size.
#[test]
fn head_reports_status() {
    let srv = server(Scenario::Atomic);
    srv.insert("headme.bin", BETA);
    let url = format!("{}headme.bin", root_url(&srv));

    let human = nxr(&["head", &url]);
    expect_exit(&human, 0, "head human");
    assert!(stdout(&human).contains("200"), "status in output");

    let json = nxr(&["--json", "head", &url]);
    expect_exit(&json, 0, "head --json");
    let lines = ndjson(&json);
    assert_eq!(lines.len(), 1, "head prints exactly one JSON object");
    assert_eq!(lines[0]["status"], 200);
    assert_eq!(lines[0]["size"], BETA.len() as u64);
    assert!(lines[0].get("url").is_some());
    assert!(lines[0].get("content_type").is_some());
}

// ---- channel --------------------------------------------------------------

/// 11. channel set/get roundtrip; --if-forward keeps the current token
///     when the new one is older.
#[test]
fn channel_set_get_and_if_forward() {
    let srv = server(Scenario::Atomic);
    let url = format!("{}channels/latest", root_url(&srv));

    let set = nxr(&["channel", "set", &url, "1.2.3"]);
    expect_exit(&set, 0, "channel set");
    assert_eq!(
        srv.store_get("channels/latest").as_deref(),
        Some(b"1.2.3\n" as &[u8])
    );

    let get = nxr(&["channel", "get", &url]);
    expect_exit(&get, 0, "channel get");
    assert_eq!(stdout(&get).trim(), "1.2.3");

    let older = nxr(&["channel", "set", "--if-forward", &url, "1.0.0"]);
    expect_exit(&older, 0, "if-forward with an older token still exits 0");
    assert_eq!(
        srv.store_get("channels/latest").as_deref(),
        Some(b"1.2.3\n" as &[u8]),
        "the current token must be kept"
    );
    assert!(stdout(&older).contains("1.2.3"), "reports the kept token");

    let newer = nxr(&["channel", "set", "--if-forward", &url, "2.0.0"]);
    expect_exit(&newer, 0, "if-forward with a newer token writes");
    assert_eq!(
        srv.store_get("channels/latest").as_deref(),
        Some(b"2.0.0\n" as &[u8])
    );
}

// ---- verify ---------------------------------------------------------------

/// 12. verify accepts a complete dir and rejects tampered bytes.
#[test]
fn verify_accepts_then_rejects_tampering() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);
    // up generates the local marker, making the dir verifiable.
    expect_exit(&nxr(&["up", src.path().to_str().unwrap(), &base]), 0, "up");

    let ok = nxr(&["verify", src.path().to_str().unwrap()]);
    expect_exit(&ok, 0, "verify of a complete dir");

    let mut tampered = ALPHA.to_vec();
    tampered[0] ^= 0xff;
    std::fs::write(src.path().join("a.zip"), &tampered).unwrap();

    let bad = nxr(&["verify", src.path().to_str().unwrap()]);
    expect_exit(&bad, 1, "verify of tampered bytes");
}

// ---- exit-code matrix -----------------------------------------------------

/// 13. Auth401: anonymous is exit 3, correct -u credentials exit 0.
///     (`head` reports a 401 as a normal status; the gate fires on fetch.)
#[test]
fn auth401_exit_codes() {
    let srv = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });
    srv.insert("file.bin", BETA);
    let url = format!("{}file.bin", root_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("file.bin");
    let anon = nxr(&["get", "--retry", "1", &url, "-o", target.to_str().unwrap()]);
    expect_exit(&anon, 3, "401 without credentials is an auth error");
    assert!(!target.exists());

    let authed = nxr(&[
        "-u",
        "nexus:secret",
        "get",
        "--retry",
        "1",
        &url,
        "-o",
        target.to_str().unwrap(),
    ]);
    expect_exit(&authed, 0, "401 scenario with the right credentials");
    assert_eq!(std::fs::read(&target).unwrap(), BETA);
}

/// 17. a name outside the grammar is misuse: exit 2.
#[test]
fn unsafe_name_is_misuse_exit_2() {
    let srv = server(Scenario::Atomic);
    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "bad!name",
    ]);
    expect_exit(&down, 2, "unsafe name is misuse");
    assert!(stderr(&down).contains("hint:"));
}

/// 18. a dead base (connection refused) is a transport error: exit 3.
#[test]
fn dead_base_exit_3() {
    // A bound-then-dropped listener: the port is closed for certain.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let out = TempDir::new().unwrap();
    let target = out.path().join("x.bin");
    let get = nxr(&[
        "get",
        &format!("http://127.0.0.1:{port}/x.bin"),
        "-o",
        target.to_str().unwrap(),
        "--retry",
        "1",
    ]);
    expect_exit(&get, 3, "connection refused is transport");
}

// ---- NDJSON ---------------------------------------------------------------

/// 14. --json on up and down: stdout is pure NDJSON with plan, artifact
///     and a final summary carrying the counters.
#[test]
fn ndjson_events_parse_and_summarize() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "b.bin", BETA);
    let base = dir_url(&srv);

    let up = nxr(&["--json", "up", src.path().to_str().unwrap(), &base]);
    expect_exit(&up, 0, "up --json");
    let events = ndjson(&up);
    assert!(
        events.iter().any(|e| e["event"] == "plan"),
        "a plan event is required: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "artifact" && e["state"] == "uploading"),
        "artifact events are required: {events:?}"
    );
    let summary = events.last().expect("at least the summary line");
    assert_eq!(summary["event"], "summary", "summary comes last");
    assert_eq!(summary["uploaded"], 2);
    assert_eq!(summary["downloaded"], 0);
    assert_eq!(summary["skipped"], 0);

    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "--json",
        "down",
        &base,
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
    ]);
    expect_exit(&down, 0, "down --json");
    let events = ndjson(&down);
    let summary = events.last().expect("at least the summary line");
    assert_eq!(summary["event"], "summary");
    assert_eq!(summary["downloaded"], 1);
    assert_eq!(summary["uploaded"], 0);
    assert!(summary.get("skipped").is_some());
}

// ---- doctor ---------------------------------------------------------------

/// A failing `up --json` still drains the event channel: stdout stays
/// complete NDJSON — plan, artifact lines — and the last line is the
/// Summary event naming both failures. The loss used to be a per-run race,
/// so the invocation repeats: five green runs in a row make the regression
/// reliably red on the old `?`-before-drain code.
///
/// The scenario fails after the diff (an exhausted cut-off upload), so the
/// events genuinely precede the error. An auth-gated up, by contrast,
/// fails inside the diff and has no events to drain: stdout is empty by
/// contract, stderr carries the failure.
#[test]
fn failing_up_still_flushes_ndjson_events() {
    let srv = server(Scenario::FreezeUpload);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "b.bin", BETA);

    for _ in 0..5 {
        let up = nxr(&[
            "--stall-secs",
            "1",
            "--retry",
            "1",
            "--json",
            "up",
            src.path().to_str().unwrap(),
            &dir_url(&srv),
        ]);
        expect_exit(&up, 3, "an exhausted cut-off upload is transport");
        assert!(
            stderr(&up).contains("hint:"),
            "a hint accompanies the failure: {}",
            stderr(&up)
        );
        let events = ndjson(&up);
        assert!(
            events.iter().any(|e| e["event"] == "plan"),
            "the plan event must survive the failure: {events:?}"
        );
        // The Summary event goes out just before the error surfaces; on the
        // old code it was the line lost most often.
        let summary = events.last().expect("at least the summary line");
        assert_eq!(
            summary["event"], "summary",
            "summary comes last: {events:?}"
        );
        let mut failed: Vec<&str> = summary["failed"]
            .as_array()
            .expect("failed is a list")
            .iter()
            .map(|v| v.as_str().expect("a name string"))
            .collect();
        failed.sort_unstable();
        assert_eq!(
            failed,
            ["a.zip", "b.bin"],
            "both names reported: {events:?}"
        );
    }

    // The auth variant: the failure precedes any event, so stdout stays
    // empty and the error lands on stderr with its hint.
    let auth = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });
    let guarded = nxr(&[
        "--json",
        "up",
        src.path().to_str().unwrap(),
        &dir_url(&auth),
        "--retry",
        "1",
    ]);
    expect_exit(&guarded, 3, "an auth-gated up is an auth failure");
    assert!(
        stderr(&guarded).contains("hint:"),
        "a hint accompanies the failure: {}",
        stderr(&guarded)
    );
    assert_eq!(
        ndjson(&guarded),
        Vec::<serde_json::Value>::new(),
        "no events precede a diff-phase failure"
    );
}

/// 15. doctor: without credentials the credentials check fails (exit 2);
///     with -u and no URL everything passes (exit 0).
#[test]
fn doctor_exit_codes() {
    let bare = nxr(&["doctor"]);
    expect_exit(&bare, 2, "doctor without credentials flags the gap");
    assert!(stderr(&bare).contains("hint:"));

    let ok = nxr(&["-u", "someone:hunter2", "doctor"]);
    expect_exit(&ok, 0, "doctor with explicit credentials passes");
    assert!(
        stdout(&ok).contains("all checks passed"),
        "got: {}",
        stdout(&ok)
    );
    assert!(
        !stdout(&ok).contains("hunter2"),
        "secrets never reach output"
    );
}

/// 15b. doctor --json: one NDJSON line per check, names and booleans only —
///      no secret ever appears in the detail field.
#[test]
fn doctor_json_lines() {
    let bare = nxr(&["--json", "doctor"]);
    expect_exit(&bare, 2, "doctor without credentials flags the gap");
    let lines = ndjson(&bare);
    assert!(
        !lines.is_empty(),
        "at least one check line is expected: {}",
        stdout(&bare)
    );
    assert!(
        lines.iter().all(|l| l.get("check").is_some()
            && l.get("ok").is_some()
            && l.get("detail").is_some()),
        "every line is a check object: {lines:?}"
    );
    let creds = lines
        .iter()
        .find(|l| l["check"] == "credentials")
        .expect("the credentials check is reported");
    assert_eq!(creds["ok"], false, "anonymous credentials fail the check");

    let ok = nxr(&["--json", "-u", "someone:hunter2", "doctor"]);
    expect_exit(&ok, 0, "doctor with explicit credentials passes");
    assert!(
        !stdout(&ok).contains("hunter2"),
        "secrets never reach json output"
    );
}
