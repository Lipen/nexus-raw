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

/// The same hermetic run with extra environment variables set on top.
fn nxr_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(NXR);
    cmd.args(args)
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("nxr binary runs")
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

/// `up --plan` prints the planned actions and transfers nothing.
#[test]
fn up_plan_prints_actions_without_uploading() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);

    let plan = nxr(&["up", "--plan", src.path().to_str().unwrap(), &dir_url(&srv)]);
    expect_exit(&plan, 0, "up --plan");
    let out = stdout(&plan);
    assert!(out.contains("upload a.zip"), "plan line missing: {out}");
    assert!(put_requests(&srv).is_empty(), "a plan must not PUT");

    // The old spelling keeps working as an alias.
    let alias = nxr(&[
        "up",
        "--dry-run",
        src.path().to_str().unwrap(),
        &dir_url(&srv),
    ]);
    expect_exit(&alias, 0, "up --dry-run alias");
    assert!(stdout(&alias).contains("upload a.zip"));

    // The JSON form is one object per plan line.
    let json = nxr(&[
        "--json",
        "up",
        "--plan",
        src.path().to_str().unwrap(),
        &dir_url(&srv),
    ]);
    expect_exit(&json, 0, "up --plan --json");
    let lines = ndjson(&json);
    assert_eq!(
        lines[0],
        serde_json::json!({"action": "upload", "name": "a.zip", "size": ALPHA.len()}),
        "the plan json shape is fixed: {lines:?}"
    );
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

/// Auth403: an unauthenticated request is refused with 403 and maps onto the auth error, exit 3, like a 401.
#[test]
fn auth403_exit_3() {
    let srv = server(Scenario::Auth403 {
        user: "nexus".into(),
        pass: "secret".into(),
    });
    srv.insert("file.bin", BETA);
    let url = format!("{}file.bin", root_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("file.bin");
    let anon = nxr(&["get", "--retry", "1", &url, "-o", target.to_str().unwrap()]);
    expect_exit(&anon, 3, "403 without credentials is an auth error");
    assert!(stderr(&anon).contains("hint:"));
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
    expect_exit(&authed, 0, "403 scenario with the right credentials");
    assert_eq!(std::fs::read(&target).unwrap(), BETA);
}

/// Redirect: the 301 is never followed and surfaces as the plain HTTP error, exit 3.
#[test]
fn redirect_exit_3() {
    let srv = server(Scenario::Redirect {
        location_path: "/repository/raw/moved".into(),
    });
    srv.insert("1.14.0/a.zip", ALPHA);
    let dst = TempDir::new().unwrap();

    let down = nxr(&[
        "down",
        "--retry",
        "1",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
    ]);
    expect_exit(&down, 3, "a 301 is an http error, not a redirect");
    assert!(stderr(&down).contains("301"));
    assert!(!dst.path().join("a.zip").exists());
}

// ---- scenario coverage ----------------------------------------------------

/// rate-limit: the run spends retries on the 429s, the NDJSON stream names them, and the upload still lands with exit 0.
#[test]
fn rate_limit_up_retries_and_succeeds() {
    let srv = server(Scenario::RateLimit {
        first_429s: 1,
        retry_after_secs: 1,
    });
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);

    let up = nxr(&["--json", "up", src.path().to_str().unwrap(), &base]);
    expect_exit(&up, 0, "up through the rate limiter");
    let events = ndjson(&up);
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "retrying" && e["name"] == "a.zip"),
        "the throttled request must surface as a retrying event: {events:?}"
    );
    let summary = events.last().expect("summary last");
    assert_eq!(summary["event"], "summary");
    assert_eq!(summary["uploaded"], 1);
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
    assert!(srv.store_get("1.14.0/a.zip.sha256").is_some());
}

/// drop-connection: the reset first request is retried on the wire and the upload lands.
#[test]
fn drop_connection_up_recovers() {
    let srv = server(Scenario::DropConnection);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);

    expect_exit(&nxr(&["up", src.path().to_str().unwrap(), &base]), 0, "up");
    // The probe was reset before anything was answered.
    assert!(
        srv.requests().iter().any(|r| r.outcome == Outcome::Reset),
        "the mock must have reset the first request: {:?}",
        srv.requests()
    );
    // The retry landed the whole artifact.
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
    assert!(srv.store_get("1.14.0/a.zip.sha256").is_some());
}

/// partial-put: the cut first PUT is retried and the bytes land whole, marker after bytes.
#[test]
fn partial_put_up_recovers() {
    let srv = server(Scenario::PartialPut {
        first_attempt_bytes: 4,
    });
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);

    expect_exit(&nxr(&["up", src.path().to_str().unwrap(), &base]), 0, "up");
    let puts = put_requests(&srv);
    let bytes_puts = puts.iter().filter(|r| r.path == "1.14.0/a.zip").count();
    assert!(bytes_puts >= 2, "the cut attempt must be retried: {puts:?}");
    // The retry stored the bytes whole and the marker followed.
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
    assert!(srv.store_get("1.14.0/a.zip.sha256").is_some());
}

/// sizeless: a 200 without Content-Length is Broken, never Absent, so the complete remote object refuses the upload with exit 1.
#[test]
fn sizeless_up_refuses_instead_of_overwriting() {
    let srv = server(Scenario::Sizeless);
    srv.insert("1.14.0/a.zip", BETA);
    srv.insert("1.14.0/a.zip.sha256", marker_line("a.zip", BETA).as_bytes());
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);

    let up = nxr(&["up", src.path().to_str().unwrap(), &base]);
    expect_exit(&up, 1, "an unverifiable remote is a data refusal");
    // The remote bytes are untouched: the refusal is not an overwrite.
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(BETA));
}

/// slow: the drip never starves a healthy run, the download is byte-perfect.
#[test]
fn slow_down_succeeds() {
    let srv = server(Scenario::Slow {
        chunk_delay_ms: 1,
        chunk_size: 3,
    });
    srv.insert("1.14.0/a.zip", ALPHA);
    let dst = TempDir::new().unwrap();

    let down = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
    ]);
    expect_exit(&down, 0, "down through the drip");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
}

/// markerless: the remote holds bare bytes, down computes the marker from the received bytes.
#[test]
fn markerless_down_writes_computed_marker() {
    let srv = server(Scenario::Markerless);
    srv.insert("1.14.0/a.zip", ALPHA);
    let dst = TempDir::new().unwrap();

    let down = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
    ]);
    expect_exit(&down, 0, "down from a markerless remote");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
    assert_eq!(
        read_file(dst.path(), "a.zip.sha256"),
        marker_line("a.zip", ALPHA).into_bytes()
    );
    assert_eq!(srv.store_get("1.14.0/a.zip.sha256"), None);
}

/// cut-body: the first GET breaks mid-body, the client retries through the part and the file lands byte-perfect.
#[test]
fn cut_body_down_completes_through_the_part() {
    let srv = server(Scenario::CutBody {
        after_bytes: 4,
        fake_length: false,
    });
    srv.insert("1.14.0/a.zip", ALPHA);
    // A complete remote (bytes plus sibling) is what makes the resume legal: without a sibling the download restarts from zero by design.
    srv.insert(
        "1.14.0/a.zip.sha256",
        marker_line("a.zip", ALPHA).as_bytes(),
    );
    let dst = TempDir::new().unwrap();

    let down = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
    ]);
    expect_exit(&down, 0, "down through a mid-body cut");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
    assert!(
        srv.requests()
            .iter()
            .any(|r| r.outcome == Outcome::Status(206)),
        "the retry must resume with Range: {:?}",
        srv.requests()
    );
}

/// flaky: one invocation absorbs the first 503s per path and lands the upload.
#[test]
fn flaky_up_recovers_in_one_invocation() {
    let srv = server(Scenario::Flaky { first_failures: 1 });
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);

    expect_exit(
        &nxr(&["up", src.path().to_str().unwrap(), &base]),
        0,
        "up through flaky",
    );
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
    assert!(srv.store_get("1.14.0/a.zip.sha256").is_some());
}

/// doc-drift: the drifted version document is an ordinary name, down fetches the ghost bytes as they come.
#[test]
fn doc_drift_down_takes_the_drifted_document() {
    let srv = server(Scenario::DocDrift);
    srv.insert(
        "1.14.0/version.json",
        br#"{"schema_version":1,"version":"1.14.0","artifacts":["real.zip"]}"#,
    );
    srv.enable_drift();
    let dst = TempDir::new().unwrap();

    let down = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "version.json",
    ]);
    expect_exit(&down, 0, "down of a drifted document");
    assert_eq!(
        read_file(dst.path(), "version.json"),
        br#"{"schema_version":1,"version":"1.14.0","artifacts":["ghost.zip"]}"#.to_vec()
    );
}

/// foreign-marker: the stored digest names a foreign object, the digest check refuses with exit 1 and drops the part.
#[test]
fn foreign_marker_down_refuses() {
    let srv = server(Scenario::ForeignMarker);
    srv.insert("1.14.0/a.zip", ALPHA);
    srv.insert(
        "1.14.0/a.zip.sha256",
        format!("{}  a.zip\n", "0".repeat(64)).as_bytes(),
    );
    let dst = TempDir::new().unwrap();

    let down = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
    ]);
    expect_exit(&down, 1, "a foreign digest is a data refusal");
    assert!(
        !dst.path().join("a.zip").exists(),
        "the refused download must not land"
    );
    assert!(
        !dst.path().join("a.zip.part").exists(),
        "the part is dropped on a mismatch"
    );
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

/// The retry surface is part of the same contract: a rate-limited up pins the `retrying` event, name, attempt and reason included.
/// The reason carries the request URL, so the test masks the ephemeral port with `PORT` before comparing; the fixture is otherwise byte-exact.
#[test]
fn golden_ndjson_retrying_holds() {
    let srv = server(Scenario::RateLimit {
        first_429s: 1,
        retry_after_secs: 1,
    });
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);

    let up = nxr(&["--json", "up", src.path().to_str().unwrap(), &base]);
    expect_exit(&up, 0, "golden retrying");
    let port = srv.addr().port().to_string();
    let normalized = stdout(&up).replace(&port, "PORT");
    assert_eq!(
        normalized,
        include_str!("golden/retrying.ndjson"),
        "the retrying ndjson changed: update tests/golden/retrying.ndjson deliberately"
    );
}

/// doctor --json is the same contract for the check-report shape.
#[test]
fn golden_ndjson_doctor_holds() {
    // The proxy row names the environment: the fixture pins the no-proxy shape.
    for v in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        std::env::remove_var(v);
    }
    let out = nxr(&["--json", "-u", "someone:hunter2", "doctor"]);
    expect_exit(&out, 0, "golden doctor");
    assert_eq!(
        stdout(&out),
        include_str!("golden/doctor.ndjson"),
        "the doctor ndjson changed: update tests/golden/doctor.ndjson deliberately"
    );
}

// ---- golden completions -----------------------------------------------------

/// The completion scripts are part of the contract, pinned byte-exact by fixtures in tests/golden/.
/// A clap tree change (a command, a flag, a help text) deliberately breaks them;
/// regenerate with `cargo run -p nexus-raw -- complete <shell> > <fixture>` and commit on purpose.
/// The scripts are deterministic: no ports, no timestamps, no environment.
#[test]
fn golden_complete_bash_holds() {
    let out = nxr(&["complete", "bash"]);
    expect_exit(&out, 0, "golden complete bash");
    assert_eq!(
        stdout(&out),
        include_str!("golden/complete-bash.txt"),
        "the bash completion changed: update tests/golden/complete-bash.txt deliberately"
    );
}

#[test]
fn golden_complete_zsh_holds() {
    let out = nxr(&["complete", "zsh"]);
    expect_exit(&out, 0, "golden complete zsh");
    assert_eq!(
        stdout(&out),
        include_str!("golden/complete-zsh.txt"),
        "the zsh completion changed: update tests/golden/complete-zsh.txt deliberately"
    );
}

#[test]
fn golden_complete_fish_holds() {
    let out = nxr(&["complete", "fish"]);
    expect_exit(&out, 0, "golden complete fish");
    assert_eq!(
        stdout(&out),
        include_str!("golden/complete-fish.txt"),
        "the fish completion changed: update tests/golden/complete-fish.txt deliberately"
    );
}

#[test]
fn golden_complete_powershell_holds() {
    let out = nxr(&["complete", "powershell"]);
    expect_exit(&out, 0, "golden complete powershell");
    assert_eq!(
        stdout(&out),
        include_str!("golden/complete-powershell.txt"),
        "the powershell completion changed: update tests/golden/complete-powershell.txt deliberately"
    );
}

/// An unknown shell is misuse: exit 2, the message names the supported four, the hint line follows.
#[test]
fn complete_unknown_shell_is_misuse_exit_2() {
    let out = nxr(&["complete", "tcsh"]);
    expect_exit(&out, 2, "complete of an unknown shell");
    assert!(
        stdout(&out).is_empty(),
        "a refused completion prints nothing to stdout"
    );
    let err = stderr(&out);
    assert!(
        err.contains("misuse: unknown shell: tcsh (use bash, zsh, fish or powershell)"),
        "the error names the shell and the supported four: {err}"
    );
    assert!(err.contains("hint:"), "the misuse hint is printed: {err}");
}

// ---- doctor ---------------------------------------------------------------

/// A failing `up --json` still drains the event channel: stdout stays complete NDJSON (plan, artifact lines) and the last line is the Summary event naming both failures.
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

/// doctor --json: one NDJSON line per check, names and booleans only, no secret ever appears in the detail field.
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

// ---- plans -----------------------------------------------------------------

/// `down --plan` prints the plan and writes nothing into the target directory.
#[test]
fn down_plan_prints_plan_without_writing() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", ALPHA);
    srv.insert(
        "1.14.0/a.zip.sha256",
        format!("{}  a.zip\n", hex_digest(ALPHA)).as_bytes(),
    );
    let dst = TempDir::new().unwrap();
    let plan = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
        "--plan",
    ]);
    expect_exit(&plan, 0, "down --plan");
    assert!(
        stdout(&plan).contains("download a.zip"),
        "got: {}",
        stdout(&plan)
    );
    assert!(!dst.path().join("a.zip").exists(), "a plan writes nothing");
}

// ---- diff ------------------------------------------------------------------

/// Seed the diff fixture: four names up'd with markers, then every delta shape
/// manufactured: two equal, one missing-local, two missing-remote, two diverged.
/// `g.zip` is put before the `up`, so the remote copy never grows a marker.
/// The second directory stays outside the scan: it feeds the remote-only put
/// and the explicit enumeration list.
fn seed_diff_fixture() -> (MockNexus, TempDir, TempDir) {
    let srv = server(Scenario::Atomic);
    let local = TempDir::new().unwrap();
    let scratch = TempDir::new().unwrap();
    let base = dir_url(&srv);

    // The remote-only markerless name: put before the up, so no sibling exists.
    write_file(
        scratch.path(),
        "g-src",
        b"gamma bytes served without a marker\n",
    );
    expect_exit(
        &nxr(&[
            "put",
            &format!("{base}g.zip"),
            "-f",
            scratch.path().join("g-src").to_str().unwrap(),
        ]),
        0,
        "seeding the remote-only g.zip",
    );

    write_file(local.path(), "a.zip", ALPHA);
    write_file(local.path(), "b.zip", ALPHA);
    write_file(local.path(), "d.zip", ALPHA);
    write_file(
        local.path(),
        "manifest.json",
        br#"{"schema_version":1,"version":"1.14.0","artifacts":["a.zip","b.zip","d.zip","manifest.json"]}"#,
    );
    expect_exit(
        &nxr(&["up", local.path().to_str().unwrap(), &base]),
        0,
        "seeding the diff fixture",
    );

    // b.zip disappears locally: missing-local.
    std::fs::remove_file(local.path().join("b.zip")).unwrap();
    std::fs::remove_file(local.path().join("b.zip.sha256")).unwrap();

    // c.zip appears locally with a correct marker: missing-remote.
    write_file(local.path(), "c.zip", BETA);
    write_file(
        local.path(),
        "c.zip.sha256",
        marker_line("c.zip", BETA).as_bytes(),
    );

    // d.zip is rewritten with the same length and a fresh marker: diverged by sha only.
    let same_len = vec![b'x'; ALPHA.len()];
    write_file(local.path(), "d.zip", &same_len);
    write_file(
        local.path(),
        "d.zip.sha256",
        marker_line("d.zip", &same_len).as_bytes(),
    );

    // g.zip appears locally, markerless, at a different size: diverged by size only.
    write_file(local.path(), "g.zip", BETA);

    // zzz.txt appears locally after the up and stays unenumerated: the union path.
    write_file(local.path(), "zzz.txt", b"local only\n");
    write_file(
        local.path(),
        "zzz.txt.sha256",
        marker_line("zzz.txt", b"local only\n").as_bytes(),
    );

    // The explicit enumeration list, g.zip included, c.zip probed away.
    write_file(
        scratch.path(),
        "list.json",
        br#"{"schema_version":1,"version":"1.14.0","artifacts":["a.zip","b.zip","c.zip","d.zip","g.zip","manifest.json"]}"#,
    );
    (srv, local, scratch)
}

/// A converged directory diffs to silence and exit 0, like diff(1).
#[test]
fn diff_equal_directory_exits_zero() {
    let srv = server(Scenario::Atomic);
    let local = TempDir::new().unwrap();
    write_file(local.path(), "a.zip", ALPHA);
    write_file(
        local.path(),
        "manifest.json",
        br#"{"schema_version":1,"version":"1.14.0","artifacts":["a.zip","manifest.json"]}"#,
    );
    let base = dir_url(&srv);
    expect_exit(
        &nxr(&["up", local.path().to_str().unwrap(), &base]),
        0,
        "seeding the equal directory",
    );

    let out = nxr(&["diff", local.path().to_str().unwrap(), &base]);
    expect_exit(&out, 0, "an equal directory is exit 0");
    assert_eq!(
        stdout(&out),
        "same a.zip\nsame manifest.json\n",
        "an equal directory reports every name as same"
    );

    // An explicit name enumerates the same convention.
    let named = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--name",
        "a.zip",
        "--name",
        "manifest.json",
    ]);
    expect_exit(&named, 0, "an equal named diff is exit 0");
}

/// `diff` prints one line per entry and changes nothing on either side.
#[test]
fn diff_reports_the_sections_and_writes_nothing() {
    let (srv, local, scratch) = seed_diff_fixture();
    let before = put_requests(&srv).len();
    let out = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &dir_url(&srv),
        "--manifest",
        scratch.path().join("list.json").to_str().unwrap(),
    ]);
    expect_exit(&out, 1, "a non-empty delta is exit 1");
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 7, "one line per entry: {text}");
    assert!(lines.contains(&"same a.zip"), "{text}");
    assert!(lines.contains(&"same manifest.json"), "{text}");
    assert!(lines.contains(&"missing-local b.zip"), "{text}");
    assert!(lines.contains(&"missing-remote c.zip"), "{text}");
    assert!(lines.contains(&"missing-remote zzz.txt"), "{text}");
    assert!(lines.contains(&"diverged d.zip (sha)"), "{text}");
    assert!(lines.contains(&"diverged g.zip (size)"), "{text}");
    assert_eq!(put_requests(&srv).len(), before, "a diff must not PUT");
    assert!(
        !srv.requests().iter().any(|r| r.method == "DELETE"),
        "a diff must not DELETE"
    );
}

/// `diff` without an enumeration source refuses like `down`, hint included.
#[test]
fn diff_without_enumeration_needs_a_source() {
    let srv = server(Scenario::Atomic);
    let local = TempDir::new().unwrap();
    write_file(local.path(), "a.zip", ALPHA);
    let out = nxr(&["diff", local.path().to_str().unwrap(), &dir_url(&srv)]);
    expect_exit(&out, 1, "cannot enumerate is exit 1");
    let err = stderr(&out);
    assert!(err.contains("cannot enumerate"), "{err}");
    assert!(err.contains("hint:"), "every error carries a hint: {err}");
}

/// `diff` misuse: a local path that is not a directory, an unsafe name on either input.
#[test]
fn diff_misuse_exits_two() {
    let srv = server(Scenario::Atomic);
    let base = dir_url(&srv);
    let file = TempDir::new().unwrap();
    write_file(file.path(), "plain.txt", b"not a directory");
    let out = nxr(&[
        "diff",
        file.path().join("plain.txt").to_str().unwrap(),
        &base,
    ]);
    expect_exit(&out, 2, "a plain file is not a directory");
    assert!(stderr(&out).contains("not a directory"));

    let out = nxr(&[
        "diff",
        file.path().to_str().unwrap(),
        &base,
        "--name",
        "bad*name",
    ]);
    expect_exit(&out, 2, "an unsafe --name is misuse");
    assert!(stderr(&out).contains("unsafe name"));

    // A local file name the grammar refuses is misuse, never a silent skip.
    let bad = TempDir::new().unwrap();
    write_file(bad.path(), "weird name.txt", b"x");
    let out = nxr(&[
        "diff",
        bad.path().to_str().unwrap(),
        &base,
        "--name",
        "a.zip",
    ]);
    expect_exit(&out, 2, "an unscannable local name is misuse");
    assert!(stderr(&out).contains("unsafe name"));
}

/// The `diff --json` surface is a contract, pinned byte-exact by fixtures in tests/golden/.
#[test]
fn golden_ndjson_diff_holds() {
    let (srv, local, scratch) = seed_diff_fixture();
    let before = put_requests(&srv).len();
    let out = nxr(&[
        "--json",
        "diff",
        local.path().to_str().unwrap(),
        &dir_url(&srv),
        "--manifest",
        scratch.path().join("list.json").to_str().unwrap(),
    ]);
    expect_exit(&out, 1, "the golden diff finds differences");
    assert_eq!(
        stdout(&out),
        include_str!("golden/diff.ndjson"),
        "the diff ndjson changed: update tests/golden/diff.ndjson deliberately"
    );
    assert_eq!(put_requests(&srv).len(), before, "a diff must not PUT");
    assert!(
        !srv.requests().iter().any(|r| r.method == "DELETE"),
        "a diff must not DELETE"
    );
}

/// `mirror --dry-run` probes both sides and prints the plan, the destination stays empty.
#[test]
fn mirror_dry_run_prints_plan_without_writing() {
    let src = server(Scenario::Atomic);
    src.insert("1.14.0/a.zip", ALPHA);
    src.insert(
        "1.14.0/manifest.json",
        br#"{"schema_version":1,"version":"1.14.0","artifacts":["a.zip"]}"#,
    );
    let dst = server(Scenario::Atomic);
    let plan = nxr(&["mirror", &dir_url(&src), &dir_url(&dst), "--dry-run"]);
    expect_exit(&plan, 0, "mirror --dry-run");
    assert!(
        stdout(&plan).contains("copy a.zip"),
        "got: {}",
        stdout(&plan)
    );
    assert!(
        dst.store_get("1.14.0/a.zip").is_none(),
        "a dry run writes nothing to the destination"
    );
}

// ---- mirror per-side credentials -------------------------------------------

fn seed_mirror_source(srv: &MockNexus) {
    srv.insert("1.14.0/a.zip", ALPHA);
    srv.insert(
        "1.14.0/a.zip.sha256",
        format!("{}  a.zip\n", hex_digest(ALPHA)).as_bytes(),
    );
    srv.insert(
        "1.14.0/manifest.json",
        br#"{"schema_version":1,"version":"1.14.0","artifacts":["a.zip"]}"#,
    );
}

/// `--dst-user` authenticates only the destination: the source stays whatever it is.
#[test]
fn mirror_dst_user_authenticates_the_destination() {
    let src = server(Scenario::Atomic);
    seed_mirror_source(&src);
    let dst = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });

    let ok = nxr(&[
        "mirror",
        &dir_url(&src),
        &dir_url(&dst),
        "--dst-user",
        "nexus:secret",
    ]);
    expect_exit(&ok, 0, "destination credentials land the copy");
    assert_eq!(dst.store_get("1.14.0/a.zip").unwrap(), ALPHA);

    // Without the flag the destination rejects the writes: nothing lands, nothing lost.
    let dst2 = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });
    let bad = nxr(&["mirror", &dir_url(&src), &dir_url(&dst2)]);
    expect_exit(&bad, 3, "no destination credentials is an auth failure");
    assert!(dst2.store_get("1.14.0/a.zip").is_none());
}

/// `--src-user` authenticates only the source: a wrong source secret never reads.
#[test]
fn mirror_src_user_authenticates_the_source() {
    let src = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });
    seed_mirror_source(&src);
    let dst = server(Scenario::Atomic);

    let ok = nxr(&[
        "mirror",
        &dir_url(&src),
        &dir_url(&dst),
        "--src-user",
        "nexus:secret",
    ]);
    expect_exit(&ok, 0, "source credentials read the source");
    assert_eq!(dst.store_get("1.14.0/a.zip").unwrap(), ALPHA);

    // A wrong source secret fails the reads: a fresh destination receives nothing.
    let dst2 = server(Scenario::Atomic);
    let bad = nxr(&[
        "mirror",
        &dir_url(&src),
        &dir_url(&dst2),
        "--src-user",
        "nexus:wrong",
    ]);
    expect_exit(&bad, 3, "a wrong source secret fails the reads");
    assert!(dst2.store_get("1.14.0/a.zip").is_none());
}

// ---- mv --------------------------------------------------------------------

/// `mv` pours the version into the destination and then empties the source, markers included.
#[test]
fn mv_moves_and_empties_the_source() {
    let src = server(Scenario::Atomic);
    seed_mirror_source(&src);
    let dst = server(Scenario::Atomic);

    let out = nxr(&["mv", &dir_url(&src), &dir_url(&dst)]);
    expect_exit(&out, 0, "a converged move is exit 0");

    // The destination holds the whole poured name.
    assert_eq!(dst.store_get("1.14.0/a.zip").unwrap(), ALPHA);
    assert!(dst.store_get("1.14.0/a.zip.sha256").is_some());
    // The source lost the bytes and the marker; the manifest survives as the enumeration source.
    assert!(src.store_get("1.14.0/a.zip").is_none());
    assert!(src.store_get("1.14.0/a.zip.sha256").is_none());
    assert!(src.store_get("1.14.0/manifest.json").is_some());
}

/// `mv --dry-run` prints both plans and moves nothing.
#[test]
fn mv_dry_run_prints_both_plans() {
    let src = server(Scenario::Atomic);
    seed_mirror_source(&src);
    let dst = server(Scenario::Atomic);

    let out = nxr(&["mv", &dir_url(&src), &dir_url(&dst), "--dry-run"]);
    expect_exit(&out, 0, "a plan is exit 0");
    let text = stdout(&out);
    assert!(text.contains("will move:"), "{text}");
    assert!(text.contains("copy a.zip"), "{text}");
    assert!(text.contains("will delete:"), "{text}");
    assert!(text.contains("rm a.zip"), "{text}");

    assert!(dst.store_get("1.14.0/a.zip").is_none(), "nothing moved");
    assert!(src.store_get("1.14.0/a.zip").is_some(), "nothing deleted");
}

/// A mirror that fails deletes nothing: the source stays whole, the destination stays empty.
#[test]
fn mv_failed_mirror_keeps_the_source() {
    let src = server(Scenario::Atomic);
    seed_mirror_source(&src);
    let dst = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });

    let out = nxr(&["mv", &dir_url(&src), &dir_url(&dst)]);
    expect_exit(&out, 3, "the pour fails on destination auth");

    assert!(
        src.store_get("1.14.0/a.zip").is_some(),
        "the source is untouched"
    );
    assert!(dst.store_get("1.14.0/a.zip").is_none(), "nothing poured");
}

/// A refused delete after a converged pour is exit 1 and a duplicate, never a loss.
#[test]
fn mv_refused_delete_leaves_a_duplicate() {
    let member = server(Scenario::Atomic);
    seed_mirror_source(&member);
    let src = MockNexus::start_group(&[&member, &server(Scenario::Atomic)]).unwrap();
    let dst = server(Scenario::Atomic);

    let out = nxr(&["mv", &dir_url(&src), &dir_url(&dst)]);
    expect_exit(&out, 1, "the read-only source refuses the delete phase");

    // The pour converged before the delete was refused: both sides hold the bytes.
    assert_eq!(dst.store_get("1.14.0/a.zip").unwrap(), ALPHA);
    assert_eq!(member.store_get("1.14.0/a.zip").unwrap(), ALPHA);
}

// ---- service repos ---------------------------------------------------------

/// `service repos` prints one `name format kind url` line per repository.
#[test]
fn service_repos_prints_human_lines() {
    let srv = server(Scenario::Atomic);
    let out = nxr(&["service", "repos", &srv.base_url()]);
    expect_exit(&out, 0, "the document parses");
    let text = stdout(&out);
    assert!(text.contains("raw-main raw hosted http://"), "{text}");
    assert!(text.contains("raw-all raw group http://"), "{text}");
}

/// `--json` is one object with the repository array; a repository URL works as the input.
#[test]
fn service_repos_json_shape_from_a_repository_url() {
    let srv = server(Scenario::Atomic);
    let inside = format!("{}repository/raw-main/1.0.0/", srv.base_url());
    let out = nxr(&["service", "repos", &inside, "--json"]);
    expect_exit(&out, 0, "a repository URL roots to the server");
    let v: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    let repos = v["repos"].as_array().expect("repos array");
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0]["name"], "raw-main");
    assert_eq!(repos[0]["format"], "raw");
    assert_eq!(repos[0]["type"], "hosted");
    assert!(repos[0]["url"].as_str().unwrap().starts_with("http://"));
}

/// A server without the service API exits 3 with the server-root hint.
#[test]
fn service_repos_missing_prints_the_root_hint() {
    let srv = server(Scenario::NoService);
    let out = nxr(&["service", "repos", &srv.base_url()]);
    expect_exit(
        &out,
        3,
        "an absent service API is a transport-class refusal",
    );
    assert!(stderr(&out).contains("server root"), "{}", stderr(&out));
}

// ---- prefix ----------------------------------------------------------------

/// `down --prefix` plans exactly the subtree; a bare prefix is misuse.
#[test]
fn down_prefix_flag_filters_the_plan() {
    let srv = server(Scenario::Atomic);
    seed_subtrees_cli(&srv);
    let dst = tempfile::tempdir().unwrap();

    let out = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--prefix",
        "bom/",
        "--dry-run",
    ]);
    expect_exit(&out, 0, "the filtered plan resolves");
    let text = stdout(&out);
    assert!(text.contains("download bom/a.zip"), "{text}");
    assert!(!text.contains("lib/c.bin"), "{text}");

    let bad = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--prefix",
        "bom",
    ]);
    expect_exit(&bad, 2, "a prefix without the slash is misuse");
    assert!(stderr(&bad).contains("whole segments"), "{}", stderr(&bad));
}

/// Two-prefix mirror pours exactly the union.
#[test]
fn mirror_prefix_flag_pours_the_union() {
    let src = server(Scenario::Atomic);
    seed_subtrees_cli(&src);
    let dst = server(Scenario::Atomic);

    let out = nxr(&[
        "mirror",
        &dir_url(&src),
        &dir_url(&dst),
        "--name",
        "bom/a.zip",
        "--name",
        "lib/c.bin",
        "--prefix",
        "bom/",
    ]);
    expect_exit(&out, 0, "the filtered pour converges");
    assert_eq!(dst.store_get("1.14.0/bom/a.zip").unwrap(), ALPHA);
    assert!(
        dst.store_get("1.14.0/lib/c.bin").is_none(),
        "the lib subtree is filtered out"
    );
}

fn seed_subtrees_cli(srv: &MockNexus) {
    srv.insert("1.14.0/bom/a.zip", ALPHA);
    srv.insert(
        "1.14.0/bom/a.zip.sha256",
        format!("{}  bom/a.zip\n", hex_digest(ALPHA)).as_bytes(),
    );
    srv.insert("1.14.0/lib/c.bin", ALPHA);
    srv.insert(
        "1.14.0/lib/c.bin.sha256",
        format!("{}  lib/c.bin\n", hex_digest(ALPHA)).as_bytes(),
    );
    srv.insert(
        "1.14.0/manifest.json",
        br#"{"schema_version":1,"version":"1.14.0","artifacts":["bom/a.zip","lib/c.bin"]}"#,
    );
}

// ---- human render: verbose format and byte-clean pipes ----------------------

/// The verbose start line names the size in human units, and a piped run stays
/// byte-clean: no ANSI escapes, no carriage-return progress, on either stream.
#[test]
fn verbose_human_output_is_formatted_and_pipe_clean() {
    let srv = server(Scenario::Atomic);
    seed_mirror_source(&srv);
    let dst = tempfile::tempdir().unwrap();

    let out = nxr(&[
        "-v",
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
    ]);
    expect_exit(&out, 0, "the verbose download lands");

    let stdout = stdout(&out);
    assert!(
        stdout.contains(&format!("↓ a.zip {} B", ALPHA.len())),
        "the start line carries a human size: {stdout}"
    );
    assert!(stdout.contains("↓ a.zip ok"), "{stdout}");
    assert!(
        !stdout.contains('\x1b'),
        "piped stdout carries no ANSI: {stdout}"
    );
    assert!(
        !stderr(&out).contains('\x1b'),
        "piped stderr carries no ANSI"
    );
    assert!(
        !stderr(&out).contains('\r'),
        "piped stderr carries no progress line"
    );
}

/// `NO_COLOR` (any non-empty value) forces plain lines even when a terminal asks for paint.
#[test]
fn no_color_forces_plain_output() {
    let srv = server(Scenario::Atomic);
    seed_mirror_source(&srv);
    let dst = tempfile::tempdir().unwrap();

    let out = nxr_env(
        &[
            "down",
            &dir_url(&srv),
            dst.path().to_str().unwrap(),
            "--name",
            "a.zip",
        ],
        &[("NO_COLOR", "1"), ("FORCE_COLOR", "1")],
    );
    expect_exit(&out, 0, "the colored run lands");
    assert!(
        !stdout(&out).contains('\x1b'),
        "NO_COLOR wins over FORCE_COLOR"
    );
}

// ---- ls: the raw tree listing ----------------------------------------------

/// `nxr ls` prints the entries of a raw directory at any depth: folders carry the slash.
#[test]
fn ls_prints_the_raw_tree_entries() {
    let srv = server(Scenario::Atomic);
    let page = concat!(
        r#"{"continuationToken":null,"items":["#,
        r#"{"path":"app/core/lib.rs"},"#,
        r#"{"path":"app/README.md"},"#,
        r#"{"path":"root.txt"}]}"#
    );
    srv.insert("service/rest/v1/search/assets", page.as_bytes());

    let out = nxr(&["ls", &format!("{}repository/raw-main/", srv.base_url())]);
    expect_exit(&out, 0, "the tree lists from the repository root");
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines, ["app/", "root.txt"], "folders first, slash-marked");

    // One level deeper: an ordinary folder behaves like any other.
    let out = nxr(&["ls", &format!("{}repository/raw-main/app/", srv.base_url())]);
    expect_exit(&out, 0, "a nested folder lists the same way");
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines, ["core/", "README.md"]);

    // The JSON shape carries the kind.
    let out = nxr(&[
        "ls",
        &format!("{}repository/raw-main/", srv.base_url()),
        "--json",
    ]);
    expect_exit(&out, 0, "the json listing resolves");
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], r#"{"entry":"app","kind":"dir"}"#);
    assert_eq!(lines[1], r#"{"entry":"root.txt","kind":"file"}"#);
}
