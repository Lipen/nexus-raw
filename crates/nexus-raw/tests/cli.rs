//! CLI conformance: the v0.3 command surface driven through the `nxr` binary against `mock-nexus`.
//!
//! Covers: up/down happy path and skip detection, marker generation (default, `--no-sha`), enumeration sources (`--name`, `--manifest`, none), Range resume, the put/get/sha/head primitives, channel refs, offline verify, auth gating, NDJSON events and the exit-code matrix.

use std::fmt::Write as _;
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

/// Run `nxr` with a hermetic environment.
/// Ambient NXR_* credentials must never leak into a test.
/// Pass `-u` explicitly instead.
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
        let _ = write!(hex, "{b:02x}");
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

/// Parse every stdout line as a JSON object.
/// Nothing else may be printed.
fn ndjson(out: &Output) -> Vec<serde_json::Value> {
    stdout(out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("line {l:?} is not JSON: {e}")))
        .collect()
}

// ---- transfers ------------------------------------------------------------

/// up then down round trip against Atomic: bytes and markers on the server, bytes and markers on disk, exit 0 everywhere.
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

/// up generates sha-siblings: the local dir starts without any marker, yet the server ends up with one (and the local dir gains it).
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

/// up --no-sha: bytes go up, no marker is generated or stored.
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

/// the second up skips everything: no new PUTs, summary all-skipped.
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

/// up --dry-run prints the plan and transfers nothing.
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

/// down with no enumeration flags and no server manifest: exit 1 with a hint about enumeration.
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

/// down --name fetches exactly the named artifact.
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

/// down --manifest with a local manifest file.
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

/// resume: a pre-seeded part file continues through a Range request.
/// The mock answers 206 and the assembled file is byte-perfect.
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

// ---- mirror ---------------------------------------------------------------

/// The version document the mirror fixtures pour.
const DOC: &[u8] = br#"{"schema_version":1,"version":"1.14.0","artifacts":["a.zip"]}"#;

/// Seed a source repository the way a publisher leaves it: bytes, markers, the version document and the manifest enumerating all three.
fn seed_publish(base: &str) {
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "version.json", DOC);
    write_file(
        src.path(),
        "manifest.json",
        br#"{"artifacts":["version.json","a.zip","manifest.json"]}"#,
    );
    let up = nxr(&["up", src.path().to_str().unwrap(), base]);
    expect_exit(&up, 0, "seeding up");
}

/// mirror with the default enumeration pours the whole version, the version document first; a rerun is a pure skip with zero content GETs.
#[test]
fn mirror_pours_the_version_and_reruns_are_pure_skips() {
    let src_srv = server(Scenario::Atomic);
    let dst_srv = server(Scenario::Atomic);
    let src_base = dir_url(&src_srv);
    let dst_base = dir_url(&dst_srv);
    seed_publish(&src_base);

    let first = nxr(&["mirror", &src_base, &dst_base]);
    expect_exit(&first, 0, "mirror with the default enumeration");
    for name in ["a.zip", "version.json", "manifest.json"] {
        assert_eq!(
            dst_srv.store_get(&format!("1.14.0/{name}")),
            src_srv.store_get(&format!("1.14.0/{name}")),
            "{name} bytes are byte-equal"
        );
        assert_eq!(
            dst_srv.store_get(&format!("1.14.0/{name}.sha256")),
            src_srv.store_get(&format!("1.14.0/{name}.sha256")),
            "{name} keeps its marker"
        );
    }
    // The enumeration led with the version document: it lands before the payloads.
    let puts: Vec<String> = dst_srv
        .requests()
        .iter()
        .filter(|r| r.method == "PUT")
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(
        puts.first().map(String::as_str),
        Some("1.14.0/version.json"),
        "the version document is claimed first: {puts:?}"
    );

    // The same command again: every name skipped, no content fetched, nothing rewritten.
    let after_first = src_srv.requests().len();
    let puts_after_first = dst_srv.put_count("1.14.0/a.zip");
    let second = nxr(&["mirror", &src_base, &dst_base]);
    expect_exit(&second, 0, "the repeated mirror");
    assert!(
        stdout(&second).contains("uploaded 0, downloaded 0, skipped 3"),
        "the rerun is a pure skip: {}",
        stdout(&second)
    );
    let content_gets = src_srv.requests()[after_first..]
        .iter()
        .filter(|r| {
            r.method == "GET"
                && ["a.zip", "version.json"]
                    .iter()
                    .any(|name| r.path == format!("1.14.0/{name}"))
        })
        .count();
    assert_eq!(content_gets, 0, "no content GETs on a converged mirror");
    assert_eq!(
        dst_srv.put_count("1.14.0/a.zip"),
        puts_after_first,
        "a converged mirror writes nothing"
    );
}

/// a complete destination object with a different digest refuses the run: exit 1, the destination is untouched.
#[test]
fn mirror_refuses_a_diverged_destination() {
    let src_srv = server(Scenario::Atomic);
    let dst_srv = server(Scenario::Atomic);
    let src_base = dir_url(&src_srv);
    let dst_base = dir_url(&dst_srv);
    seed_publish(&src_base);

    // The destination holds a complete object under the same name with other bytes.
    let foreign = TempDir::new().unwrap();
    write_file(foreign.path(), "a.zip", BETA);
    let put = nxr(&[
        "put",
        &format!("{dst_base}a.zip"),
        "-f",
        foreign.path().join("a.zip").to_str().unwrap(),
        "--sha",
    ]);
    expect_exit(&put, 0, "seeding the diverged destination");

    let out = nxr(&["mirror", &src_base, &dst_base, "--name", "a.zip"]);
    expect_exit(&out, 1, "divergence is a data refusal");
    assert!(
        stderr(&out).contains("mismatch") && stderr(&out).contains("hint:"),
        "the refusal names the divergence and carries a hint: {stderr}",
        stderr = stderr(&out)
    );
    assert_eq!(
        dst_srv.store_get("1.14.0/a.zip").unwrap(),
        BETA,
        "the destination bytes survive the refused run"
    );
}

// ---- primitives -----------------------------------------------------------

/// put → get roundtrip, put --sha stores the sibling, sha prints the digest of the remote bytes.
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

/// head reports the status.
/// JSON mode carries status and size.
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

/// channel set/get roundtrip.
/// `--if-forward` keeps the current token when the new one is older.
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

/// verify accepts a complete dir and rejects tampered bytes.
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

/// Auth401: anonymous is exit 3, correct -u credentials exit 0.
/// (`head` reports a 401 as a normal status.
/// The gate fires on fetch.)
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

/// a name outside the grammar is misuse: exit 2.
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

/// a dead base (connection refused) is a transport error: exit 3.
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

/// --json on up and down: stdout is pure NDJSON with plan, artifact and a final summary carrying the counters.
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

// ---- golden ndjson --------------------------------------------------------

/// The --json surface is a contract, pinned byte-exact by fixtures in tests/golden/.
/// One artifact per run keeps chunk events deterministic.
/// No URL reaches these outputs, so the bytes carry no ports.
#[test]
fn golden_ndjson_up_down_hold() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);

    let up = nxr(&["--json", "up", src.path().to_str().unwrap(), &base]);
    expect_exit(&up, 0, "golden up");
    assert_eq!(
        stdout(&up),
        include_str!("golden/up.ndjson"),
        "the up ndjson changed: update tests/golden/up.ndjson deliberately"
    );

    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "--json",
        "down",
        &base,
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
    ]);
    expect_exit(&down, 0, "golden down");
    assert_eq!(
        stdout(&down),
        include_str!("golden/down.ndjson"),
        "the down ndjson changed: update tests/golden/down.ndjson deliberately"
    );
}

/// The mirror shares the transfer event schema: the same shapes, one stream, pinned byte-exact.
#[test]
fn golden_ndjson_mirror_holds() {
    let src_srv = server(Scenario::Atomic);
    let dst_srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let src_base = dir_url(&src_srv);
    let dst_base = dir_url(&dst_srv);
    let up = nxr(&["up", src.path().to_str().unwrap(), &src_base]);
    expect_exit(&up, 0, "seeding the mirror golden");

    let mirror = nxr(&["--json", "mirror", &src_base, &dst_base, "--name", "a.zip"]);
    expect_exit(&mirror, 0, "golden mirror");
    assert_eq!(
        stdout(&mirror),
        include_str!("golden/mirror.ndjson"),
        "the mirror ndjson changed: update tests/golden/mirror.ndjson deliberately"
    );
}

/// doctor --json is the same contract for the check-report shape.
#[test]
fn golden_ndjson_doctor_holds() {
    let out = nxr(&["--json", "-u", "someone:hunter2", "doctor"]);
    expect_exit(&out, 0, "golden doctor");
    assert_eq!(
        stdout(&out),
        include_str!("golden/doctor.ndjson"),
        "the doctor ndjson changed: update tests/golden/doctor.ndjson deliberately"
    );
}

// ---- doctor ---------------------------------------------------------------

/// A failing `up --json` still drains the event channel: stdout stays complete NDJSON — plan, artifact lines — and the last line is the Summary event naming both failures.
///
/// The drain is a race, so one green run proves nothing: the invocation repeats five times.
///
/// The scenario fails after the diff (an exhausted cut-off upload), so the events genuinely precede the error.
/// An auth-gated up, by contrast, fails inside the diff and has no events to drain: stdout is empty by contract, stderr carries the failure.
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
        // The Summary event goes out just before the failure, and the error object
        // closes the stream: the machine channel sees the failure too.
        let summary = &events[events.len() - 2];
        assert_eq!(
            summary["event"], "summary",
            "summary precedes the error object: {events:?}"
        );
        let last = events.last().expect("the error object closes the stream");
        assert_eq!(last["event"], "error", "got: {last}");
        assert_eq!(last["code"], 3, "got: {last}");
        assert!(
            last["hint"].is_string(),
            "the error object carries its hint: {last}"
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

    // The auth variant: the failure precedes any transfer event, so the only line
    // on stdout is the closing error object, and stderr repeats it with the hint.
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
    let lines = ndjson(&guarded);
    assert_eq!(
        lines.len(),
        1,
        "the error object is the only line: {lines:?}"
    );
    assert_eq!(lines[0]["event"], "error");
    assert_eq!(lines[0]["code"], 3);
    assert!(
        lines[0]["hint"].as_str().unwrap().contains("NXR_AUTH"),
        "the auth hint rides the error object: {lines:?}"
    );
}

/// doctor: missing credentials are a warning (exit 0), explicit credentials pass clean (exit 0).
/// A broken setting (retry 0) is a failure (exit 2).
#[test]
fn doctor_exit_codes() {
    let bare = nxr(&["doctor"]);
    expect_exit(&bare, 0, "missing credentials warn without failing");
    assert!(stdout(&bare).contains("[ warn ]"));
    assert!(stdout(&bare).contains("all checks passed"));

    let ok = nxr(&["-u", "someone:hunter2", "doctor"]);
    expect_exit(&ok, 0, "doctor with explicit credentials passes");
    assert!(
        stdout(&ok).contains("all checks passed"),
        "got: {}",
        stdout(&ok)
    );
    assert!(!stdout(&ok).contains("[ warn ]"));
    assert!(
        !stdout(&ok).contains("hunter2"),
        "secrets never reach output"
    );

    let broken = nxr(&["--retry", "0", "doctor"]);
    expect_exit(&broken, 2, "a setting the commands refuse fails the check");
    assert!(stderr(&broken).contains("hint:"));
}

/// doctor --json: one NDJSON line per check, names and booleans only — no secret ever appears in the detail field.
/// Missing credentials carry `ok: true` plus the additive `warn` field.
#[test]
fn doctor_json_lines() {
    let bare = nxr(&["--json", "doctor"]);
    expect_exit(&bare, 0, "missing credentials warn without failing");
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
    assert_eq!(
        creds["ok"], true,
        "anonymous access is a warning, not a failure"
    );
    assert_eq!(creds["warn"], true, "the warning is visible in json");

    let ok = nxr(&["--json", "-u", "someone:hunter2", "doctor"]);
    expect_exit(&ok, 0, "doctor with explicit credentials passes");
    let ok_lines = ndjson(&ok);
    let creds = ok_lines
        .iter()
        .find(|l| l["check"] == "credentials")
        .expect("the credentials check is reported");
    assert_eq!(
        creds["warn"],
        serde_json::Value::Null,
        "no warn field without a warning"
    );
    assert!(
        !stdout(&ok).contains("hunter2"),
        "secrets never reach json output"
    );
}

/// doctor against an auth server: the probe turns 401 into a credentials failure (exit 3),
/// and correct credentials turn the same probe green.
#[test]
fn doctor_probe_rejects_rejected_credentials() {
    let srv = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });
    let url = root_url(&srv);

    let bad = nxr(&["--json", "doctor", &url]);
    expect_exit(&bad, 3, "a rejected probe is an auth verdict");
    let lines = ndjson(&bad);
    let probe = lines
        .iter()
        .find(|l| l["check"] == "probe")
        .expect("the probe is reported");
    assert_eq!(probe["ok"], false);
    assert!(
        probe["detail"]
            .as_str()
            .unwrap()
            .contains("credentials rejected"),
        "got: {probe}"
    );

    let good = nxr(&["--json", "-u", "nexus:secret", "doctor", &url]);
    expect_exit(&good, 0, "valid credentials pass the probe");
    let lines = ndjson(&good);
    let probe = lines
        .iter()
        .find(|l| l["check"] == "probe")
        .expect("the probe is reported");
    assert_eq!(probe["ok"], true, "got: {probe}");
}

// ---- rm and point ---------------------------------------------------------

/// Seed the version directory through `up` and install the enumeration manifest plus the version document.
fn publish_for_rm(srv: &MockNexus, manifest: &str) -> String {
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "b.bin", BETA);
    let base = dir_url(srv);
    expect_exit(
        &nxr(&["up", src.path().to_str().unwrap(), &base]),
        0,
        "up for rm",
    );
    srv.insert("1.14.0/version.json", br#"{"version":"1.14.0"}"#);
    srv.insert("1.14.0/manifest.json", manifest.as_bytes());
    base
}

/// rm deletes the whole enumerated version: names, markers and the version document, manifest excluded; a rerun sees only 404s and exits 0.
#[test]
fn rm_removes_whole_version_and_reruns_clean() {
    let srv = server(Scenario::Atomic);
    let base = publish_for_rm(&srv, r#"{"artifacts":["a.zip","b.bin","version.json"]}"#);

    let rm = nxr(&["rm", &base]);
    expect_exit(&rm, 0, "first rm");
    for name in [
        "a.zip",
        "a.zip.sha256",
        "b.bin",
        "b.bin.sha256",
        "version.json",
    ] {
        assert!(
            srv.store_get(&format!("1.14.0/{name}")).is_none(),
            "{name} must be gone"
        );
    }
    assert!(
        srv.store_get("1.14.0/manifest.json").is_some(),
        "the enumeration manifest survives: the rerun needs it"
    );
    let out = stdout(&rm);
    assert!(out.contains("× a.zip removed"), "per-name lines: {out}");
    assert!(out.contains("removed 3, skipped 0"), "summary: {out}");

    // Marker before bytes, the reverse of publishing.
    let log = srv.requests();
    for name in ["a.zip", "b.bin"] {
        let marker_at = log
            .iter()
            .position(|r| r.method == "DELETE" && r.path == format!("1.14.0/{name}.sha256"))
            .unwrap();
        let bytes_at = log
            .iter()
            .position(|r| r.method == "DELETE" && r.path == format!("1.14.0/{name}"))
            .unwrap();
        assert!(marker_at < bytes_at, "{name}: marker first");
    }

    let second = nxr(&["rm", &base]);
    expect_exit(&second, 0, "the rerun is a normal 404 walk");
    assert!(
        stdout(&second).contains("○ version.json missing"),
        "the rerun reports missing names: {}",
        stdout(&second)
    );
    assert!(
        stdout(&second).contains("skipped 3"),
        "summary: {}",
        stdout(&second)
    );
    let deletes: Vec<u16> = srv
        .requests()
        .iter()
        .skip(log.len())
        .filter(|r| r.method == "DELETE")
        .filter_map(|r| match r.outcome {
            mock_nexus::Outcome::Status(s) => Some(s),
            _ => None,
        })
        .collect();
    assert!(
        !deletes.is_empty() && deletes.iter().all(|s| *s == 404),
        "{deletes:?}"
    );
}

/// The read-only scenario refuses rm: exit 1, a hint, and the mock counts zero successful deletes.
#[test]
fn rm_readonly_refuses_and_keeps_bytes() {
    let srv = server(Scenario::ReadOnly);
    let base = publish_for_rm(&srv, r#"{"artifacts":["a.zip","b.bin"]}"#);

    let rm = nxr(&["rm", &base]);
    expect_exit(&rm, 1, "read-only refusal is exit 1");
    let err = stderr(&rm);
    assert!(
        err.contains("read-only"),
        "the error names the refusal: {err}"
    );
    assert!(err.contains("hint:"), "a hint line is required: {err}");

    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
    assert_eq!(
        srv.store_get("1.14.0/a.zip.sha256").as_deref(),
        Some(marker_line("a.zip", ALPHA).as_bytes()),
    );
    let successful = srv.requests().iter().any(|r| {
        r.method == "DELETE" && matches!(r.outcome, mock_nexus::Outcome::Status(200..=300))
    });
    assert!(!successful, "no DELETE may succeed");
}

/// rm --dry-run prints the plan and the mock counts zero DELETEs.
#[test]
fn rm_dry_run_touches_no_bytes() {
    let srv = server(Scenario::Atomic);
    let base = publish_for_rm(
        &srv,
        r#"{"artifacts":["a.zip","b.bin","version.json","ghost.bin"]}"#,
    );

    let dry = nxr(&["rm", "--dry-run", &base]);
    expect_exit(&dry, 0, "rm --dry-run");
    let out = stdout(&dry);
    assert!(out.contains("rm a.zip"), "plan line: {out}");
    assert!(
        out.contains("missing ghost.bin"),
        "absent names plan as missing: {out}"
    );
    assert!(
        !srv.requests().iter().any(|r| r.method == "DELETE"),
        "a dry run must not DELETE"
    );
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
}

/// rm without any enumeration source refuses like down: exit 1 with the enumeration hint.
#[test]
fn rm_without_a_source_refuses() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", ALPHA);

    let rm = nxr(&["rm", &dir_url(&srv)]);
    expect_exit(&rm, 1, "rm without a source");
    let err = stderr(&rm);
    assert!(
        err.contains("enumerat"),
        "the error names enumeration: {err}"
    );
    assert!(err.contains("hint:"), "{err}");
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
}

/// rm --json: NDJSON events removing/removed/missing per name, the summary line last.
#[test]
fn rm_json_events_parse_and_summarize() {
    let srv = server(Scenario::Atomic);
    let base = publish_for_rm(&srv, r#"{"artifacts":["a.zip","b.bin","version.json"]}"#);

    let rm = nxr(&["--json", "rm", &base]);
    expect_exit(&rm, 0, "rm --json");
    let events = ndjson(&rm);
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "removing" && e["name"] == "a.zip"),
        "removing events are required: {events:?}"
    );
    assert_eq!(
        events.iter().filter(|e| e["event"] == "removed").count(),
        3,
        "one removed event per name: {events:?}"
    );
    let summary = events.last().expect("at least the summary line");
    assert_eq!(summary["event"], "summary", "summary comes last");
    assert_eq!(summary["removed"], 3);
    assert_eq!(summary["skipped"], 0);
    assert_eq!(summary["uploaded"], 0);

    // The second run emits missing events and still succeeds.
    let second = nxr(&["--json", "rm", &base]);
    expect_exit(&second, 0, "rm --json rerun");
    let events = ndjson(&second);
    assert_eq!(
        events.iter().filter(|e| e["event"] == "missing").count(),
        3,
        "missing events for an empty version: {events:?}"
    );
    assert_eq!(events.last().unwrap()["removed"], 0);
}

/// Exit-code matrix rows for rm: unsafe name 2, dead base 3 (readonly and no-source are 1, covered above).
#[test]
fn rm_exit_matrix_rows() {
    let srv = server(Scenario::Atomic);
    let bad = nxr(&["rm", &dir_url(&srv), "--name", "bad!name"]);
    expect_exit(&bad, 2, "unsafe name is misuse");
    assert!(stderr(&bad).contains("hint:"));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let dead = nxr(&[
        "rm",
        &format!("http://127.0.0.1:{port}/1.0.0/"),
        "--name",
        "a.zip",
        "--retry",
        "1",
    ]);
    expect_exit(&dead, 3, "a dead base is transport");
}

/// point --clear deletes a channel ref and stays idempotent; the read-only scenario refuses it.
#[test]
fn point_clear_roundtrip_and_readonly() {
    let srv = server(Scenario::Atomic);
    let url = format!("{}stable", root_url(&srv));
    expect_exit(&nxr(&["channel", "set", &url, "1.4.0"]), 0, "channel set");

    let clear = nxr(&["point", "--clear", &url]);
    expect_exit(&clear, 0, "first clear");
    assert!(stdout(&clear).contains("cleared"), "{}", stdout(&clear));
    assert!(srv.store_get("stable").is_none());

    let again = nxr(&["point", "--clear", &url]);
    expect_exit(&again, 0, "clearing an absent pointer is exit 0");
    assert!(stdout(&again).contains("absent"), "{}", stdout(&again));

    // `point` without --clear is misuse.
    let bare = nxr(&["point", &url]);
    expect_exit(&bare, 2, "point without --clear is misuse");
    assert!(stderr(&bare).contains("hint:"));

    let ro = server(Scenario::ReadOnly);
    ro.insert("stable", b"1.4.0\n");
    let ro_url = format!("{}stable", root_url(&ro));
    let refused = nxr(&["point", "--clear", &ro_url]);
    expect_exit(&refused, 1, "read-only refusal");
    assert!(
        stderr(&refused).contains("read-only"),
        "{}",
        stderr(&refused)
    );
    assert!(ro.store_get("stable").is_some(), "the pointer must survive");
}
