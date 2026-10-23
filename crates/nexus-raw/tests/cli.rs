//! CLI conformance: the v0.3 command surface driven through the `nxr` binary against `mock-nexus`.
//!
//! Covers: up/down happy path and skip detection, marker generation (default, `--no-sha`), enumeration sources (`--name`, none), Range resume, the put/get/sha/head primitives, channel refs, offline verify, auth gating, NDJSON events and the exit-code matrix.

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
const GAMMA: &[u8] = b"gamma payload: the mutated successor\n";

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
        .env_remove("NXR_CA_BUNDLE")
        .output()
        .expect("nxr binary runs")
}

/// The same hermetic run with `stdin` piped in: for `--names-from -`.
fn nxr_stdin(args: &[&str], stdin: &str) -> Output {
    use std::io::Write;
    let mut child = Command::new(NXR)
        .args(args)
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD")
        .env_remove("NXR_CA_BUNDLE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("nxr binary spawns");
    child
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("nxr binary runs")
}

/// The same hermetic run with extra environment variables set on top.
fn nxr_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(NXR);
    cmd.args(args)
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD")
        .env_remove("NXR_CA_BUNDLE");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("nxr binary runs")
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn stdout(out: &Output) -> String {
    // A windows runner hands the child's output over with CRLF: the tests
    // judge content, not the platform's line-ending convention.
    String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n")
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

/// The two-subtree tree the `up --prefix` tests filter: `a/x.bin` and `b/y.bin`.
fn seed_prefix_tree(src: &TempDir) {
    std::fs::create_dir_all(src.path().join("a")).unwrap();
    std::fs::create_dir_all(src.path().join("b")).unwrap();
    write_file(src.path(), "a/x.bin", ALPHA);
    write_file(src.path(), "b/y.bin", BETA);
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

/// A clap parse error is misuse 2 with a hint, and `--json` carries the same failure.
#[test]
fn a_bad_flag_is_misuse_with_a_hint() {
    let out = nxr(&["--bogus"]);
    expect_exit(&out, 2, "an unknown flag is misuse");
    assert!(stderr(&out).contains("hint:"), "got: {}", stderr(&out));

    let json = nxr(&["--json", "--bogus"]);
    expect_exit(&json, 2, "an unknown flag is misuse in json mode too");
    let lines = ndjson(&json);
    let last = lines.last().expect("an error object");
    assert_eq!(last["event"], "error");
    assert_eq!(last["code"], 2);
    assert!(
        last["hint"].as_str().unwrap_or_default().contains("--help"),
        "got: {last}"
    );
    assert!(
        stderr(&json).is_empty(),
        "the json channel carries the failure alone: {}",
        stderr(&json)
    );
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

/// A nested name keeps its relative path in the marker, on both sides: the shape another implementation publishes,
/// and the one `sha256sum -c` resolves from the transferred root instead of only from the artifact's directory.
#[test]
fn up_marker_names_the_nested_path() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    std::fs::create_dir_all(src.path().join("bom")).unwrap();
    write_file(src.path(), "bom/a.zip", ALPHA);

    let up = nxr(&["up", src.path().to_str().unwrap(), &dir_url(&srv)]);
    expect_exit(&up, 0, "up with a nested name");
    assert_eq!(
        srv.store_get("1.14.0/bom/a.zip.sha256").as_deref(),
        Some(marker_line("bom/a.zip", ALPHA).as_bytes()),
        "the remote marker names the artifact relative to the directory being transferred"
    );
    assert_eq!(
        read_file(src.path(), "bom/a.zip.sha256"),
        marker_line("bom/a.zip", ALPHA).into_bytes(),
        "the generated local sibling carries the same name"
    );
}

/// up --no-sha: bytes go up, no marker is generated or stored.
#[test]
fn up_rejects_a_part_staging_file() {
    let srv = server(Scenario::Atomic);
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("data.part"), b"x").unwrap();
    let out = nxr(&["up", src.path().to_str().unwrap(), &dir_url(&srv)]);
    expect_exit(&out, 2, "a part staging file is misuse");
    assert!(stderr(&out).contains(".part"), "{}", stderr(&out));
}

#[test]
#[cfg(unix)]
fn up_rejects_a_symlinked_entry() {
    // An interior `latest -> 1.4.0` link used to vanish from the upload.
    // The scan now refuses it before any byte moves.
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    std::fs::create_dir_all(src.path().join("1.4.0")).unwrap();
    write_file(src.path(), "1.4.0/a.zip", ALPHA);
    std::os::unix::fs::symlink(src.path().join("1.4.0"), src.path().join("latest")).unwrap();
    let out = nxr(&["up", src.path().to_str().unwrap(), &dir_url(&srv)]);
    expect_exit(&out, 2, "a symlink is misuse");
    assert!(stderr(&out).contains("symlink"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("regular files or directories"),
        "the hint must name the non-regular rule: {}",
        stderr(&out)
    );
    assert!(
        put_requests(&srv).is_empty(),
        "the refusal must precede the wire"
    );
}

#[test]
fn up_prefix_uploads_only_the_selected_tree() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    seed_prefix_tree(&src);
    let up = nxr(&[
        "up",
        src.path().to_str().unwrap(),
        &dir_url(&srv),
        "--prefix",
        "a/",
    ]);
    expect_exit(&up, 0, "up --prefix");
    assert_eq!(srv.store_get("1.14.0/a/x.bin").as_deref(), Some(ALPHA));
    assert!(srv.store_get("1.14.0/b/y.bin").is_none());
    assert!(src.path().join("a/x.bin.sha256").exists());
    assert!(
        !src.path().join("b/y.bin.sha256").exists(),
        "a filtered-out name must not get a local marker"
    );
}

#[test]
fn up_prefix_plan_shows_only_the_selected_tree() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    seed_prefix_tree(&src);
    let plan = nxr(&[
        "up",
        src.path().to_str().unwrap(),
        &dir_url(&srv),
        "--prefix",
        "a/",
        "--plan",
    ]);
    expect_exit(&plan, 0, "up --prefix --plan");
    let text = stdout(&plan);
    assert!(text.contains("a/x.bin"), "{text}");
    assert!(!text.contains("b/y.bin"), "{text}");
    assert!(put_requests(&srv).is_empty());
}

#[test]
fn up_prefix_refuses_a_claim_outside() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    seed_prefix_tree(&src);
    let out = nxr(&[
        "up",
        src.path().to_str().unwrap(),
        &dir_url(&srv),
        "--prefix",
        "a/",
        "--claim-first",
        "b/y.bin",
    ]);
    expect_exit(&out, 2, "claim outside the prefix");
    assert!(
        stderr(&out).contains("outside the selected prefixes"),
        "{}",
        stderr(&out)
    );
    assert!(put_requests(&srv).is_empty());
}

#[test]
fn up_prefix_with_no_matches_is_misuse() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    seed_prefix_tree(&src);
    let out = nxr(&[
        "up",
        src.path().to_str().unwrap(),
        &dir_url(&srv),
        "--prefix",
        "z/",
    ]);
    expect_exit(&out, 2, "a missing prefix directory");
    assert!(
        stderr(&out).contains("prefix is not a directory"),
        "{}",
        stderr(&out)
    );
}

#[test]
#[cfg(unix)]
fn up_prefix_ignores_an_unselected_symlink() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    seed_prefix_tree(&src);
    std::os::unix::fs::symlink(src.path().join("a"), src.path().join("b/link")).unwrap();
    let up = nxr(&[
        "up",
        src.path().to_str().unwrap(),
        &dir_url(&srv),
        "--prefix",
        "a/",
    ]);
    expect_exit(&up, 0, "a symlink outside the prefix");
    assert_eq!(srv.store_get("1.14.0/a/x.bin").as_deref(), Some(ALPHA));
    assert!(srv.store_get("1.14.0/b/y.bin").is_none());
}

#[test]
#[cfg(unix)]
fn up_prefix_refuses_a_symlinked_prefix_root() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    std::fs::create_dir_all(src.path().join("a")).unwrap();
    write_file(src.path(), "a/x.bin", ALPHA);
    std::os::unix::fs::symlink(src.path().join("a"), src.path().join("b")).unwrap();
    let out = nxr(&[
        "up",
        src.path().to_str().unwrap(),
        &dir_url(&srv),
        "--prefix",
        "b/",
    ]);
    expect_exit(&out, 2, "a symlinked prefix root");
    assert!(stderr(&out).contains("symlink"), "{}", stderr(&out));
    assert!(put_requests(&srv).is_empty());
}

#[test]
#[cfg(unix)]
fn up_prefix_refuses_a_symlinked_source_root() {
    let srv = server(Scenario::Atomic);
    let real = TempDir::new().unwrap();
    std::fs::create_dir_all(real.path().join("a")).unwrap();
    write_file(real.path(), "a/x.bin", ALPHA);
    let link = real.path().join("link");
    std::os::unix::fs::symlink(real.path(), &link).unwrap();
    let out = nxr(&[
        "up",
        link.to_str().unwrap(),
        &dir_url(&srv),
        "--prefix",
        "a/",
    ]);
    expect_exit(&out, 2, "a symlinked source root");
    assert!(stderr(&out).contains("symlink"), "{}", stderr(&out));
}

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

/// A file URL with no enumeration source is a one-name download into `DST`.
#[test]
fn down_file_url_fetches_one_artifact() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = format!("{}repo/dir/", srv.base_url());
    expect_exit(&nxr(&["up", src.path().to_str().unwrap(), &base]), 0, "up");

    let dst = TempDir::new().unwrap();
    let file_url = format!("{base}a.zip");
    let down = nxr(&["down", &file_url, dst.path().to_str().unwrap()]);
    expect_exit(&down, 0, "down file url");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
    assert!(dst.path().join("a.zip.sha256").exists());
}

/// `down --segments` splits each artifact over closed Range requests and lands
/// the same bytes as one stream.
#[test]
fn down_segments_matches_the_single_stream() {
    let srv = server(Scenario::Atomic);
    let base = dir_url(&srv);
    let src = TempDir::new().unwrap();
    let content: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    write_file(src.path(), "big.bin", &content);
    expect_exit(
        &nxr(&["up", src.path().to_str().unwrap(), &base]),
        0,
        "seeding up",
    );

    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "down",
        &base,
        dst.path().to_str().unwrap(),
        "--name",
        "big.bin",
        "--segments",
        "4",
    ]);
    expect_exit(&down, 0, "down --segments");
    assert_eq!(
        read_file(dst.path(), "big.bin"),
        content,
        "the assembled spans must be byte-exact"
    );
    let ranged = srv
        .requests()
        .iter()
        .filter(|r| {
            r.method == "GET"
                && r.path == "1.14.0/big.bin"
                && matches!(r.outcome, Outcome::Status(206))
        })
        .count();
    assert_eq!(
        ranged,
        4,
        "four spans must ride four Range requests: {:?}",
        srv.requests()
    );
}

/// A segment count outside 2..=64 is misuse: exit 2, before any transfer.
#[test]
fn down_segments_bounds_are_misuse() {
    let srv = server(Scenario::Atomic);
    let dst = TempDir::new().unwrap();
    let out = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
        "--segments",
        "1",
    ]);
    expect_exit(&out, 2, "down --segments 1");
    assert!(stderr(&out).contains("segments"), "{}", stderr(&out));
}

// ---- clone ----------------------------------------------------------------

fn clone_repo_url(srv: &MockNexus) -> String {
    format!("{}repository/raw-main/", srv.base_url())
}

/// `clone` copies a whole repository, writes the fact cache, and a second run
/// converges without downloading anything.
#[test]
fn clone_copies_a_repository_and_skips_on_the_second_run() {
    let srv = server(Scenario::SearchLag { lag_ms: 0 });
    let base = clone_repo_url(&srv);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    std::fs::create_dir_all(src.path().join("sub")).unwrap();
    write_file(&src.path().join("sub"), "b.json", BETA);
    expect_exit(
        &nxr(&["up", src.path().to_str().unwrap(), &base]),
        0,
        "seeding up",
    );

    let dst = TempDir::new().unwrap();
    let out = nxr(&["clone", &base, dst.path().to_str().unwrap()]);
    expect_exit(&out, 0, "clone");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
    assert_eq!(read_file(&dst.path().join("sub"), "b.json"), BETA);
    assert!(
        dst.path().join(".nxr-state.json").is_file(),
        "the default cache lands in the destination"
    );

    let out = nxr(&["clone", &base, dst.path().to_str().unwrap()]);
    expect_exit(&out, 0, "second clone");
    assert!(
        stdout(&out).contains("downloaded 0"),
        "a converged clone downloads nothing: {}",
        stdout(&out)
    );
}

/// `clone --plan` prints the actions and writes nothing.
#[test]
fn clone_plan_writes_nothing() {
    let srv = server(Scenario::SearchLag { lag_ms: 0 });
    let base = clone_repo_url(&srv);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    expect_exit(
        &nxr(&["up", src.path().to_str().unwrap(), &base]),
        0,
        "seeding up",
    );

    let dst = TempDir::new().unwrap();
    let out = nxr(&["clone", &base, dst.path().to_str().unwrap(), "--plan"]);
    expect_exit(&out, 0, "clone --plan");
    assert!(stdout(&out).contains("download a.zip"), "{}", stdout(&out));
    assert_eq!(
        std::fs::read_dir(dst.path()).unwrap().count(),
        0,
        "a plan writes nothing"
    );
}

/// `clone --json` carries the additive `sealed` count in the summary.
#[test]
fn clone_json_summary_carries_sealed() {
    let srv = server(Scenario::SearchLag { lag_ms: 0 });
    let base = clone_repo_url(&srv);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    expect_exit(
        &nxr(&["up", src.path().to_str().unwrap(), &base]),
        0,
        "seeding up",
    );

    let dst = TempDir::new().unwrap();
    let out = nxr(&["clone", &base, dst.path().to_str().unwrap(), "--json"]);
    expect_exit(&out, 0, "clone --json");
    assert!(
        stdout(&out).contains(r#""event":"summary""#),
        "{}",
        stdout(&out)
    );
    assert!(stdout(&out).contains(r#""sealed":0"#), "{}", stdout(&out));
}

/// A foreign manifest.json on the server is ignored: enumeration comes only from flags.
/// down without --name/--ls refuses even when a manifest-looking file exists.
#[test]
fn down_ignores_a_foreign_manifest() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", ALPHA);
    srv.insert("1.14.0/manifest.json", b"{\"artifacts\":[\"a.zip\"]}");
    let dst = TempDir::new().unwrap();

    let down = nxr(&["down", &dir_url(&srv), dst.path().to_str().unwrap()]);
    expect_exit(&down, 1, "down ignores a foreign manifest");
    assert!(!dst.path().join("a.zip").exists());
}

/// --names-from reads a file of names, one per line, and downloads exactly those.
#[test]
fn down_names_from_file() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "b.bin", BETA);
    let base = dir_url(&srv);
    expect_exit(&nxr(&["up", src.path().to_str().unwrap(), &base]), 0, "up");

    let list = TempDir::new().unwrap();
    let list_path = list.path().join("names.txt");
    std::fs::write(&list_path, "a.zip\n\nb.bin\n").unwrap();

    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "down",
        &base,
        dst.path().to_str().unwrap(),
        "--names-from",
        list_path.to_str().unwrap(),
    ]);
    expect_exit(&down, 0, "down --names-from file");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
    assert_eq!(read_file(dst.path(), "b.bin"), BETA);
}

/// `--names-from -` reads the name list from stdin.
#[test]
fn down_names_from_stdin() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);
    expect_exit(&nxr(&["up", src.path().to_str().unwrap(), &base]), 0, "up");

    let dst = TempDir::new().unwrap();
    let down = nxr_stdin(
        &[
            "down",
            &base,
            dst.path().to_str().unwrap(),
            "--names-from",
            "-",
        ],
        "a.zip\n",
    );
    expect_exit(&down, 0, "down --names-from -");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
}

/// --name and --names-from combine, duplicates are the same name once.
#[test]
fn down_names_from_combines_with_argv() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "b.bin", BETA);
    let base = dir_url(&srv);
    expect_exit(&nxr(&["up", src.path().to_str().unwrap(), &base]), 0, "up");

    let list = TempDir::new().unwrap();
    let list_path = list.path().join("names.txt");
    std::fs::write(&list_path, "a.zip\n").unwrap();

    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "down",
        &base,
        dst.path().to_str().unwrap(),
        "--name",
        "b.bin",
        "--names-from",
        list_path.to_str().unwrap(),
    ]);
    expect_exit(&down, 0, "down --name plus --names-from");
    assert_eq!(read_file(dst.path(), "a.zip"), ALPHA);
    assert_eq!(read_file(dst.path(), "b.bin"), BETA);
}

/// An empty --names-from file is no enumeration source: the run refuses (exit 1).
#[test]
fn down_names_from_empty_refuses() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", ALPHA);
    let list = TempDir::new().unwrap();
    let list_path = list.path().join("names.txt");
    std::fs::write(&list_path, "\n  \n").unwrap();
    let dst = TempDir::new().unwrap();

    let down = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--names-from",
        list_path.to_str().unwrap(),
    ]);
    expect_exit(&down, 1, "empty names file refuses");
    let err = stderr(&down);
    assert!(
        err.contains("no --name/--names-from/--ls given"),
        "the refusal names all three sources: {err}"
    );
}

/// A line that fails the name grammar is a misuse (exit 2) naming the file and the line.
#[test]
fn down_names_from_bad_line_is_misuse() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", ALPHA);
    let list = TempDir::new().unwrap();
    let list_path = list.path().join("names.txt");
    std::fs::write(&list_path, "a.zip\nbad//name\n").unwrap();
    let dst = TempDir::new().unwrap();

    let down = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--names-from",
        list_path.to_str().unwrap(),
    ]);
    expect_exit(&down, 2, "bad file line is misuse");
    let err = stderr(&down);
    assert!(
        err.contains("line 2") && err.contains("bad//name"),
        "the error names the file line: {err}"
    );
}

/// `--continue` needs a target: resume writes a sibling part file.
#[test]
fn get_continue_without_output_is_misuse() {
    let srv = server(Scenario::Atomic);
    srv.insert("a.bin", ALPHA);
    let url = format!("{}a.bin", root_url(&srv));
    let out = nxr(&["get", "--continue", &url]);
    expect_exit(&out, 2, "resume without a target is misuse");
    assert!(stderr(&out).contains("--continue"), "{}", stderr(&out));
}

/// `-q` keeps the summary line and drops the per-event output.
#[test]
fn quiet_prints_only_the_summary_line() {
    let srv = server(Scenario::Atomic);
    let base = dir_url(&srv);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let out = nxr(&["-q", "up", src.path().to_str().unwrap(), &base]);
    expect_exit(&out, 0, "quiet up");
    let text = stdout(&out);
    assert!(text.contains("uploaded 1"), "the summary remains: {text}");
    assert_eq!(
        text.lines().filter(|line| !line.trim().is_empty()).count(),
        1,
        "exactly one line: {text}"
    );
}

/// The TLS flag is reported by doctor.
/// A zero timeout is refused by the same config validation.
#[test]
fn tls_insecure_is_reported_and_zero_timeout_is_misuse() {
    // Doctor treats the flag as a finding: the point is that the setting reaches it.
    let off = nxr(&["--tls-insecure", "doctor"]);
    expect_exit(&off, 2, "tls-insecure is a doctor finding");
    assert!(
        stdout(&off).contains("verification is OFF"),
        "{}",
        stdout(&off)
    );
    let zero = nxr(&["--connect-timeout-secs", "0", "doctor"]);
    expect_exit(&zero, 2, "a zero timeout is misuse");
    assert!(stderr(&zero).contains("hint:"), "{}", stderr(&zero));
}

/// `rm --names-from` deletes exactly the listed names.
#[test]
fn rm_names_from_deletes_only_the_list() {
    let srv = server(Scenario::Atomic);
    let base = dir_url(&srv);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "b.bin", BETA);
    expect_exit(
        &nxr(&["up", src.path().to_str().unwrap(), &base]),
        0,
        "seed the delete list",
    );
    let list = TempDir::new().unwrap();
    let names = list.path().join("names.txt");
    std::fs::write(&names, "a.zip\n").unwrap();
    let out = nxr(&[
        "rm",
        &base,
        "--names-from",
        names.to_str().unwrap(),
        "--yes",
    ]);
    expect_exit(&out, 0, "the listed name is removed");
    assert!(srv.store_get("1.14.0/a.zip").is_none(), "a.zip bytes");
    assert!(
        srv.store_get("1.14.0/a.zip.sha256").is_none(),
        "a.zip marker"
    );
    assert!(srv.store_get("1.14.0/b.bin").is_some(), "b.bin stays");
}

/// A plain get ignores an existing `<out>.part`: only `--continue` resumes.
#[test]
fn plain_get_ignores_an_existing_part() {
    let srv = server(Scenario::Atomic);
    srv.insert("plain.bin", ALPHA);
    let url = format!("{}plain.bin", root_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("plain.bin");
    let part = out.path().join("plain.bin.part");
    std::fs::write(&part, b"stale-bytes").unwrap();

    let got = nxr(&["get", &url, "-o", target.to_str().unwrap()]);
    expect_exit(&got, 0, "a plain get must not resume");
    assert_eq!(std::fs::read(&target).unwrap(), ALPHA);
    assert!(!part.exists(), "the transfer consumes its part");
}

/// A missing --names-from file is a misuse (exit 2), not a transport error.
#[test]
fn down_names_from_missing_file_is_misuse() {
    let srv = server(Scenario::Atomic);
    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--names-from",
        "/definitely/not/a/file.txt",
    ]);
    expect_exit(&down, 2, "missing names file is misuse");
    assert!(stderr(&down).contains("cannot read --names-from"));
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

/// A cut transfer keeps the bytes it landed: the rerun the transport hint
/// recommends resumes from the part instead of starting over.
#[test]
fn failed_get_keeps_the_part_for_the_rerun() {
    let srv = server(Scenario::CutBody {
        after_bytes: 100,
        fake_length: false,
    });
    let content: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
    srv.insert("big.bin", &content);
    let url = format!("{}big.bin", root_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("big.bin");
    let part = out.path().join("big.bin.part");

    let failed = nxr(&[
        "get",
        "--continue",
        &url,
        "-o",
        target.to_str().unwrap(),
        "--retry",
        "1",
    ]);
    expect_exit(&failed, 3, "the cut body is transport");
    assert!(
        part.exists(),
        "a failed transfer must keep its partial file for the rerun"
    );
    assert_eq!(
        std::fs::read(&part).unwrap().len(),
        100,
        "the part holds the bytes that landed before the cut"
    );
    assert!(!target.exists(), "nothing is renamed before the digest");

    let again = nxr(&["get", "--continue", &url, "-o", target.to_str().unwrap()]);
    expect_exit(&again, 0, "the rerun resumes the retained part");
    assert_eq!(std::fs::read(&target).unwrap(), content, "byte-perfect");
    assert!(!part.exists(), "the completed part is renamed away");
}

/// A symlink at the predictable `.part` path is never read as resume state
/// and never becomes the renamed output.
#[test]
#[cfg(unix)]
fn get_continue_refuses_a_symlinked_part() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", ALPHA);
    let url = format!("{}a.zip", dir_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("a.zip");
    let part = out.path().join("a.zip.part");
    let protected = out.path().join("protected.bin");
    std::fs::write(&protected, b"keep me").unwrap();
    std::os::unix::fs::symlink(&protected, &part).unwrap();

    let got = nxr(&["get", "--continue", &url, "-o", target.to_str().unwrap()]);
    assert!(
        !got.status.success(),
        "a symlinked part must fail, not be followed"
    );
    assert_eq!(std::fs::read(&protected).unwrap(), b"keep me");
    assert!(!target.exists(), "no output is renamed from a symlink");
}

/// A part longer than the object answers 416: the client must prove it holds
/// the whole object before finalizing, and restart from zero when it does not.
#[test]
fn get_continue_restarts_an_oversized_part() {
    let srv = server(Scenario::Atomic);
    let content: Vec<u8> = (0..1_000u32).map(|i| (i % 251) as u8).collect();
    srv.insert("big.bin", &content);
    let url = format!("{}big.bin", root_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("big.bin");
    let part = out.path().join("big.bin.part");
    std::fs::write(&part, vec![0xEEu8; 2_000]).unwrap();

    let got = nxr(&[
        "get",
        "--continue",
        &url,
        "-o",
        target.to_str().unwrap(),
        "--retry",
        "1",
    ]);
    expect_exit(&got, 0, "the stale part restarts and completes");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        content,
        "a 416 never blesses bytes the object does not carry"
    );
    assert!(!part.exists());
}

// ---- mirror ---------------------------------------------------------------

/// The version document the mirror fixtures pour.
const DOC: &[u8] = br#"{"schema_version":1,"version":"1.14.0","artifacts":["a.zip"]}"#;

/// Seed a source repository the way a publisher leaves it: bytes, markers and the version document.
fn seed_publish(base: &str) {
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "version.json", DOC);
    let up = nxr(&["up", src.path().to_str().unwrap(), base]);
    expect_exit(&up, 0, "seeding up");
}

/// A mirror with explicit names pours exactly those names.
/// A rerun is a pure skip with zero content GETs.
#[test]
fn mirror_pours_the_version_and_reruns_are_pure_skips() {
    let src_srv = server(Scenario::Atomic);
    let dst_srv = server(Scenario::Atomic);
    let src_base = dir_url(&src_srv);
    let dst_base = dir_url(&dst_srv);
    seed_publish(&src_base);

    let first = nxr(&[
        "mirror",
        &src_base,
        &dst_base,
        "--name",
        "a.zip",
        "--name",
        "version.json",
    ]);
    expect_exit(&first, 0, "mirror with explicit names");
    for name in ["a.zip", "version.json"] {
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
    // The same command again: every name skipped, no content fetched, nothing rewritten.
    let after_first = src_srv.requests().len();
    let puts_after_first = dst_srv.put_count("1.14.0/a.zip");
    let second = nxr(&[
        "mirror",
        &src_base,
        &dst_base,
        "--name",
        "a.zip",
        "--name",
        "version.json",
    ]);
    expect_exit(&second, 0, "the repeated mirror");
    assert!(
        stdout(&second).contains("uploaded 0, downloaded 0, skipped 2"),
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

/// `put --stdin` buffers the pipe, sends one PUT, `--sha` stores the sibling:
/// the streamed body roundtrips exactly, the marker matches the bytes.
#[test]
fn put_stdin_roundtrip_with_sha_sibling() {
    let srv = server(Scenario::Atomic);
    let root = root_url(&srv);

    let plain_url = format!("{root}pub/pipe.bin");
    let body = std::str::from_utf8(ALPHA).unwrap();
    let put = nxr_stdin(&["put", &plain_url, "--stdin"], body);
    expect_exit(&put, 0, "plain put --stdin");
    assert_eq!(srv.store_get("pub/pipe.bin").as_deref(), Some(ALPHA));
    assert!(srv.store_get("pub/pipe.bin.sha256").is_none());

    let marked_url = format!("{root}pub/pipe-marked.bin");
    let put_sha = nxr_stdin(&["put", "--sha", &marked_url, "--stdin"], body);
    expect_exit(&put_sha, 0, "put --stdin --sha");
    assert_eq!(
        srv.store_get("pub/pipe-marked.bin.sha256").as_deref(),
        Some(marker_line("pipe-marked.bin", ALPHA).as_bytes()),
    );

    // Both -f and --stdin is a misuse refusal, not a silent winner.
    let file = TempDir::new().unwrap();
    write_file(file.path(), "blob.bin", ALPHA);
    let both = nxr(&[
        "put",
        &format!("{root}pub/both.bin"),
        "-f",
        file.path().join("blob.bin").to_str().unwrap(),
        "--stdin",
    ]);
    expect_exit(&both, 2, "put with both -f and --stdin refuses");
    assert!(
        stderr(&both).contains("cannot be used with"),
        "the refusal names the conflict: {}",
        stderr(&both)
    );

    // Neither is the same class: the command cannot send nothing.
    let neither = nxr(&["put", &format!("{root}pub/none.bin")]);
    expect_exit(&neither, 2, "put with no source refuses");
}

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

/// A 404 head is a result, not an error: the status is the answer.
#[test]
fn head_404_is_a_result_not_an_error() {
    let srv = server(Scenario::Atomic);
    let url = format!("{}missing.bin", root_url(&srv));

    let human = nxr(&["head", &url]);
    expect_exit(&human, 0, "a 404 head is a result");
    assert!(stdout(&human).contains("404"), "{}", stdout(&human));

    let json = nxr(&["--json", "head", &url]);
    expect_exit(&json, 0, "a 404 head is a result in json too");
    let lines = ndjson(&json);
    assert_eq!(lines[0]["status"], 404);
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

// ---- aliases (-R) ----------------------------------------------------------

/// A hermetic alias file next to the mock: `mock` carries env credentials,
/// `anon` carries none (anonymous through the alias is legal).
fn alias_config(srv: &MockNexus, dir: &tempfile::TempDir) -> std::path::PathBuf {
    let cfg = dir.path().join("nxr");
    std::fs::create_dir_all(&cfg).unwrap();
    let file = cfg.join("config.toml");
    std::fs::write(
        &file,
        format!(
            "[alias.mock]\nurl = \"{}\"\nuser_env = \"TEST_ALIAS_USER\"\npass_env = \"TEST_ALIAS_PASS\"\n\n[alias.anon]\nurl = \"{}\"\n",
            srv.base_url(),
            srv.base_url()
        ),
    )
    .unwrap();
    file
}

fn nxr_r(args: &[&str], cfg_dir: &tempfile::TempDir, env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(NXR);
    cmd.args(args)
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD")
        .env_remove("NXR_CA_BUNDLE")
        .env("XDG_CONFIG_HOME", cfg_dir.path());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("nxr binary runs")
}

/// The same invocation run from `cwd`: a relative argument under `-R` must be
/// judged against the directory the user is actually in.
fn nxr_r_in(
    args: &[&str],
    cfg_dir: &tempfile::TempDir,
    env: &[(&str, &str)],
    cwd: &std::path::Path,
) -> Output {
    let mut cmd = Command::new(NXR);
    cmd.current_dir(cwd)
        .args(args)
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD")
        .env_remove("NXR_CA_BUNDLE")
        .env("XDG_CONFIG_HOME", cfg_dir.path());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("nxr binary runs")
}

/// -R expands a relative path argument onto the alias base and authenticates
/// through the env variables the alias names: the round trip works end to end.
#[test]
fn alias_put_get_roundtrip() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.0.0/host.txt", ALPHA);
    let cfg = TempDir::new().unwrap();
    alias_config(&srv, &cfg);

    let out = TempDir::new().unwrap();
    let target = out.path().join("host.txt");
    let get = nxr_r(
        &[
            "-R",
            "mock",
            "get",
            "1.0.0/host.txt",
            "-o",
            target.to_str().unwrap(),
        ],
        &cfg,
        &[("TEST_ALIAS_USER", "u"), ("TEST_ALIAS_PASS", "p")],
    );
    expect_exit(&get, 0, "get through the alias");
    assert_eq!(std::fs::read(&target).unwrap(), ALPHA);
}

/// An absolute URL argument passes through unchanged: the alias stays out of the way.
#[test]
fn alias_absolute_url_passes_through() {
    let srv = server(Scenario::Atomic);
    let cfg = TempDir::new().unwrap();
    alias_config(&srv, &cfg);

    let url = format!("{}1.0.0/missing.bin", srv.base_url());
    let out = TempDir::new().unwrap();
    let get = nxr_r(
        &[
            "-R",
            "mock",
            "get",
            &url,
            "-o",
            out.path().join("m.bin").to_str().unwrap(),
        ],
        &cfg,
        &[("TEST_ALIAS_USER", "u"), ("TEST_ALIAS_PASS", "p")],
    );
    expect_exit(&get, 1, "the absolute URL is honored: 404 is data");
}

/// A bare alias (no credential source) runs anonymous: the server decides.
#[test]
fn alias_anonymous_is_legal() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.0.0/a.bin", ALPHA);
    let cfg = TempDir::new().unwrap();
    alias_config(&srv, &cfg);

    let out = TempDir::new().unwrap();
    let target = out.path().join("a.bin");
    let get = nxr_r(
        &[
            "-R",
            "anon",
            "get",
            "1.0.0/a.bin",
            "-o",
            target.to_str().unwrap(),
        ],
        &cfg,
        &[],
    );
    expect_exit(&get, 0, "the bare alias runs anonymously");
    assert_eq!(std::fs::read(&target).unwrap(), ALPHA);
}

/// A missing env variable the alias names is a misuse error naming the variable.
#[test]
fn alias_missing_env_names_the_variable() {
    let srv = server(Scenario::Atomic);
    let cfg = TempDir::new().unwrap();
    alias_config(&srv, &cfg);

    let get = nxr_r(
        &["-R", "mock", "get", "1.0.0/a.bin", "-o", "x.bin"],
        &cfg,
        &[],
    );
    expect_exit(&get, 2, "the env source is broken");
    let err = stderr(&get);
    assert!(err.contains("TEST_ALIAS_USER"), "{err}");
}

/// An unknown alias lists the known ones.
#[test]
fn alias_unknown_lists_known() {
    let srv = server(Scenario::Atomic);
    let cfg = TempDir::new().unwrap();
    alias_config(&srv, &cfg);

    let ls = nxr_r(&["-R", "nope", "ls", "."], &cfg, &[]);
    expect_exit(&ls, 2, "an unknown alias is misuse");
    let err = stderr(&ls);
    assert!(err.contains("known: anon, mock"), "{err}");
}

/// Without -R no config file is read: a broken config file does not touch the call.
#[test]
fn no_r_flag_never_reads_the_file() {
    let srv = server(Scenario::Atomic);
    let cfg = TempDir::new().unwrap();
    let cfgdir = cfg.path().join("nxr");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(cfgdir.join("config.toml"), "this is not toml [").unwrap();

    srv.insert("1.0.0/a.bin", ALPHA);
    let out = TempDir::new().unwrap();
    let target = out.path().join("a.bin");
    let get = nxr_r(
        &[
            "get",
            &format!("{}1.0.0/a.bin", srv.base_url()),
            "-o",
            target.to_str().unwrap(),
        ],
        &cfg,
        &[],
    );
    expect_exit(&get, 0, "the broken file is invisible without -R");
}

/// -u wins over the alias credentials: the wrong alias env is never consulted.
#[test]
fn u_flag_wins_over_alias() {
    let srv = server(Scenario::Auth401 {
        user: "real".into(),
        pass: "pass".into(),
    });
    srv.insert("1.0.0/a.bin", BETA);
    let cfg = TempDir::new().unwrap();
    alias_config(&srv, &cfg);

    let out = TempDir::new().unwrap();
    let target = out.path().join("a.bin");
    let get = nxr_r(
        &[
            "-R",
            "mock",
            "-u",
            "real:pass",
            "get",
            "1.0.0/a.bin",
            "-o",
            target.to_str().unwrap(),
        ],
        &cfg,
        // The alias env would fail the call if it were consulted first.
        &[],
    );
    expect_exit(&get, 0, "-u overrides the alias credentials");
    assert_eq!(std::fs::read(&target).unwrap(), BETA);
}

/// NXR_CONFIG relocations the file: the default location stays untouched.
#[test]
fn nxr_config_env_relocates_the_file() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.0.0/a.bin", ALPHA);
    let cfg = TempDir::new().unwrap();
    let file = cfg.path().join("elsewhere.toml");
    std::fs::write(
        &file,
        format!("[alias.mock]\nurl = \"{}\"\n", srv.base_url()),
    )
    .unwrap();

    let out = TempDir::new().unwrap();
    let target = out.path().join("a.bin");
    let mut cmd = Command::new(NXR);
    cmd.args([
        "-R",
        "mock",
        "get",
        "1.0.0/a.bin",
        "-o",
        target.to_str().unwrap(),
    ])
    .env("NXR_CONFIG", &file)
    .env_remove("XDG_CONFIG_HOME")
    .env_remove("HOME")
    .env_remove("NXR_AUTH")
    .env_remove("NXR_USERNAME")
    .env_remove("NXR_PASSWORD");
    let get = cmd.output().expect("nxr binary runs");
    expect_exit(&get, 0, "NXR_CONFIG names the file");
    assert_eq!(std::fs::read(&target).unwrap(), ALPHA);
}

/// A mirror through two aliases: both URLs expand, the mirror pours for real.
#[test]
fn alias_mirror_two_remotes() {
    let src_srv = server(Scenario::Atomic);
    let dst_srv = server(Scenario::Atomic);
    seed_publish(&dir_url(&src_srv));
    let cfg = TempDir::new().unwrap();
    let cfgdir = cfg.path().join("nxr");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        format!(
            "[alias.src]\nurl = \"{}\"\n\n[alias.dst]\nurl = \"{}\"\n",
            src_srv.base_url(),
            dst_srv.base_url()
        ),
    )
    .unwrap();

    let mirror = nxr_r(
        &[
            "-R",
            "src",
            "mirror",
            "1.14.0/",
            &format!("{}1.14.0/", dst_srv.base_url()),
            "--name",
            "a.zip",
            "--name",
            "version.json",
        ],
        &cfg,
        &[],
    );
    expect_exit(&mirror, 0, "the mirror pours through two aliases");
    let check = TempDir::new().unwrap();
    let check_path = check.path().join("alias-mirror-check.bin");
    let back = nxr(&[
        "get",
        &format!("{}1.14.0/a.zip", dst_srv.base_url()),
        "-o",
        check_path.to_str().unwrap(),
    ]);
    expect_exit(&back, 0, "the artifact landed on the destination");
    assert_eq!(std::fs::read(&check_path).unwrap(), ALPHA);
}

/// doctor through an alias: the URL expands, and the credential source line
/// names the alias, never claims `-u` (the argv-exposure audit stays honest).
#[test]
fn alias_doctor_expands_and_names_the_source() {
    let srv = server(Scenario::Atomic);
    let cfg = TempDir::new().unwrap();
    alias_config(&srv, &cfg);

    let doc = nxr_r(
        &["-R", "mock", "doctor", "."],
        &cfg,
        &[("TEST_ALIAS_USER", "u"), ("TEST_ALIAS_PASS", "p")],
    );
    expect_exit(&doc, 0, "doctor through the alias");
    let out = String::from_utf8_lossy(&doc.stdout);
    assert!(out.contains("resolved from the -R alias"), "{out}");
    assert!(!out.contains("resolved from -u flag"), "{out}");
}

/// `sha` through `-R`: a relative URL expands onto the alias base, while an
/// existing local path stays a local path and is hashed without any request.
#[test]
fn alias_sha_expands_a_url_and_keeps_a_local_path() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", ALPHA);
    let cfg = TempDir::new().unwrap();
    alias_config(&srv, &cfg);

    let remote = nxr_r(
        &["-R", "mock", "sha", "1.14.0/a.zip"],
        &cfg,
        &[("TEST_ALIAS_USER", "u"), ("TEST_ALIAS_PASS", "p")],
    );
    expect_exit(&remote, 0, "sha through the alias");
    let after_remote = srv.requests().len();

    let local_dir = TempDir::new().unwrap();
    std::fs::write(local_dir.path().join("a.zip"), ALPHA).unwrap();
    let local = nxr_r_in(
        &["-R", "mock", "sha", "a.zip"],
        &cfg,
        &[("TEST_ALIAS_USER", "u"), ("TEST_ALIAS_PASS", "p")],
        local_dir.path(),
    );
    expect_exit(&local, 0, "a relative local path is hashed locally");

    assert_eq!(
        stdout(&remote),
        stdout(&local),
        "the remote and local copies are the same bytes"
    );
    assert_eq!(
        srv.requests().len(),
        after_remote,
        "a local sha must not send a request"
    );
}

// ---- scenario coverage ----------------------------------------------------

/// `sha` refuses a missing local file and a non-http URL as misuse (exit 2),
/// the class the exit-code table promises.
#[test]
fn sha_misuse_is_exit_2() {
    let missing = nxr(&["sha", "/no/such/local-file.bin"]);
    expect_exit(&missing, 2, "a missing local file is misuse");
    assert!(
        stderr(&missing).contains("no such local file"),
        "{}",
        stderr(&missing)
    );

    let ftp = nxr(&["sha", "ftp://example.com/x.bin"]);
    expect_exit(&ftp, 2, "a non-http URL is misuse");
    assert!(
        stderr(&ftp).contains("http(s)"),
        "the refusal names the accepted schemes: {}",
        stderr(&ftp)
    );
}

/// The atomic scenario serves no such object: a GET of a missing name is a
/// storage 404, a data fact (exit 1), not a transport failure (exit 3).
/// Scripts branch on the class: 0 proceed, 1 absent, 3 retry later.
#[test]
fn get_range_reads_the_exact_span() {
    let srv = server(Scenario::Atomic);
    let payload: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    srv.insert("1.14.0/blob.bin", payload);
    let url = format!("{}1.14.0/blob.bin", srv.base_url());
    let out = TempDir::new().unwrap();
    let dst = out.path().join("span.bin");
    let got = nxr(&["get", &url, "--range", "2-5", "-o", dst.to_str().unwrap()]);
    expect_exit(&got, 0, "get --range");
    assert_eq!(std::fs::read(&dst).unwrap(), &payload[2..=5]);
    assert!(stdout(&got).contains("range 2-5"), "{}", stdout(&got));
}

#[test]
fn get_range_open_ended_reads_the_tail() {
    let srv = server(Scenario::Atomic);
    let payload: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    srv.insert("1.14.0/blob.bin", payload);
    let url = format!("{}1.14.0/blob.bin", srv.base_url());
    let out = TempDir::new().unwrap();
    let dst = out.path().join("tail.bin");
    let got = nxr(&["get", &url, "--range", "5-", "-o", dst.to_str().unwrap()]);
    expect_exit(&got, 0, "get --range open ended");
    assert_eq!(std::fs::read(&dst).unwrap(), &payload[5..]);
}

#[test]
fn get_range_to_stdout_writes_only_the_span() {
    let srv = server(Scenario::Atomic);
    let payload: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    srv.insert("1.14.0/blob.bin", payload);
    let url = format!("{}1.14.0/blob.bin", srv.base_url());
    let got = nxr(&["get", &url, "--range", "2-5"]);
    expect_exit(&got, 0, "get --range to stdout");
    assert_eq!(got.stdout, payload[2..=5]);
}

#[test]
fn get_range_survives_a_range_ignoring_server() {
    let srv = server(Scenario::IgnoreRange);
    let payload: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    srv.insert("1.14.0/blob.bin", payload);
    let url = format!("{}1.14.0/blob.bin", srv.base_url());
    let out = TempDir::new().unwrap();
    let dst = out.path().join("span.bin");
    let got = nxr(&["get", &url, "--range", "2-5", "-o", dst.to_str().unwrap()]);
    expect_exit(&got, 0, "get --range on an ignoring server");
    assert_eq!(std::fs::read(&dst).unwrap(), &payload[2..=5]);
}

#[test]
fn get_range_past_the_end_is_misuse() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/blob.bin", ALPHA);
    let url = format!("{}1.14.0/blob.bin", srv.base_url());
    let out = TempDir::new().unwrap();
    let dst = out.path().join("span.bin");
    let got = nxr(&[
        "get",
        &url,
        "--range",
        "100-200",
        "-o",
        dst.to_str().unwrap(),
    ]);
    expect_exit(&got, 2, "a range past the end");
    assert!(stderr(&got).contains("past the end"), "{}", stderr(&got));
    assert!(!dst.exists(), "no file for an unsatisfiable range");
}

#[test]
#[cfg(unix)]
fn get_range_never_follows_a_symlinked_output() {
    let srv = server(Scenario::Atomic);
    let payload: &[u8] = b"0123456789";
    srv.insert("1.14.0/blob.bin", payload);
    let url = format!("{}1.14.0/blob.bin", srv.base_url());
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("target.bin");
    std::fs::write(&target, b"keep me").unwrap();
    let link = dir.path().join("out.bin");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let got = nxr(&["get", &url, "--range", "2-5", "-o", link.to_str().unwrap()]);
    expect_exit(&got, 0, "range to a symlinked output");
    assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
    assert_eq!(std::fs::read(&link).unwrap(), b"2345");
    let meta = std::fs::symlink_metadata(&link).unwrap();
    assert!(
        meta.file_type().is_file(),
        "the output must be replaced, not followed"
    );
}

#[test]
fn get_range_missing_object_is_data() {
    let srv = server(Scenario::Atomic);
    let url = format!("{}1.14.0/absent.bin", srv.base_url());
    let got = nxr(&["get", &url, "--range", "0-1"]);
    expect_exit(&got, 1, "a missing range target is data");
}

#[test]
fn get_range_with_segments_is_misuse() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/blob.bin", ALPHA);
    let url = format!("{}1.14.0/blob.bin", srv.base_url());
    let out = TempDir::new().unwrap();
    let dst = out.path().join("span.bin");
    let got = nxr(&[
        "get",
        &url,
        "--range",
        "2-5",
        "--segments",
        "2",
        "-o",
        dst.to_str().unwrap(),
    ]);
    expect_exit(&got, 2, "range and segments conflict");
}

#[test]
fn ca_bundle_missing_file_is_misuse() {
    let out = nxr(&[
        "get",
        "http://127.0.0.1:9/x",
        "--ca-bundle",
        "/nonexistent/ca.pem",
    ]);
    expect_exit(&out, 2, "missing CA bundle");
    assert!(stderr(&out).contains("--ca-bundle"), "{}", stderr(&out));
}

#[test]
fn ca_bundle_garbage_is_misuse() {
    let dir = TempDir::new().unwrap();
    let pem = dir.path().join("ca.pem");
    std::fs::write(&pem, b"not a certificate\n").unwrap();
    let out = nxr(&[
        "get",
        "http://127.0.0.1:9/x",
        "--ca-bundle",
        pem.to_str().unwrap(),
    ]);
    expect_exit(&out, 2, "unparseable CA bundle");
    assert!(stderr(&out).contains("--ca-bundle"), "{}", stderr(&out));
}

#[test]
fn ca_bundle_env_is_read() {
    let out = nxr_env(
        &["get", "http://127.0.0.1:9/x"],
        &[("NXR_CA_BUNDLE", "/nonexistent/ca.pem")],
    );
    expect_exit(&out, 2, "NXR_CA_BUNDLE");
    assert!(stderr(&out).contains("--ca-bundle"), "{}", stderr(&out));
}

#[test]
fn help_groups_local_and_global_flags() {
    let out = nxr(&["up", "--help"]);
    expect_exit(&out, 0, "up help");
    let text = stdout(&out);
    let local = text.find("--prefix").expect("the local flag is listed");
    let global = text
        .find("Global options")
        .expect("the global section is titled");
    assert!(local < global, "local flags come first:\n{text}");
    assert!(
        text[global..].contains("--ca-bundle"),
        "global flags follow the heading:\n{text}"
    );
    assert!(
        !text[..global].contains("--ca-bundle"),
        "global flags do not sit among the local ones:\n{text}"
    );
}

#[test]
fn ca_bundle_env_value_is_hidden_from_help() {
    let out = nxr_env(&["--help"], &[("NXR_CA_BUNDLE", "/tmp/private-ca.pem")]);
    expect_exit(&out, 0, "help with the CA env");
    let text = stdout(&out);
    assert!(text.contains("NXR_CA_BUNDLE"), "{text}");
    assert!(
        !text.contains("/tmp/private-ca.pem"),
        "the env value must not leak into help: {text}"
    );
}

#[test]
fn ca_bundle_conflicts_with_tls_insecure() {
    let out = nxr(&[
        "get",
        "http://127.0.0.1:9/x",
        "--ca-bundle",
        "/tmp/ca.pem",
        "--tls-insecure",
    ]);
    expect_exit(&out, 2, "contradictory trust flags");
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn get_missing_object_is_data_not_transport() {
    let srv = server(Scenario::Atomic);
    let url = format!("{}1.14.0/no-such-object.bin", dir_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("no-such-object.bin");
    let get = nxr(&["get", "--retry", "1", &url, "-o", target.to_str().unwrap()]);
    expect_exit(&get, 1, "a storage 404 is a data error");
    assert!(stderr(&get).contains("http 404"));
    assert!(stderr(&get).contains("hint:"));
    assert!(!target.exists());
}

/// A resumed part is validated against the `.sha256` sibling the server
/// carries: garbage in the part diverges after the download and the run
/// refuses with exit 1, keeping nothing. The quiet-corruption guard.
#[test]
fn get_resume_validates_against_the_sibling() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", BETA);
    srv.insert(
        "1.14.0/a.zip.sha256",
        format!("{}\n", test_hex(BETA)).as_bytes(),
    );
    let url = format!("{}a.zip", dir_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("a.zip");
    // A foreign garbage part shorter than the object: the resume appends the
    // real tail, so only the sibling digest can tell the mix from the object.
    // (A part longer than the object is caught earlier, at the 416.)
    let part = out.path().join("a.zip.part");
    std::fs::write(&part, vec![0xEEu8; 10]).unwrap();

    let get = nxr(&["get", "--continue", &url, "-o", target.to_str().unwrap()]);
    expect_exit(&get, 1, "the diverging resume is refused");
    assert!(stderr(&get).contains("diverges"), "{}", stderr(&get));
    // Nothing lands: the corrupted bytes never reach the target path.
    assert!(!target.exists());
}

/// A matching part resumes normally: the guard never fires on honest parts.
#[test]
fn get_resume_with_matching_part_succeeds() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", BETA);
    srv.insert(
        "1.14.0/a.zip.sha256",
        format!("{}\n", test_hex(BETA)).as_bytes(),
    );
    let url = format!("{}a.zip", dir_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("a.zip");
    // An honest prefix of the artifact.
    std::fs::write(out.path().join("a.zip.part"), &BETA[..8]).unwrap();

    let get = nxr(&["get", "--continue", &url, "-o", target.to_str().unwrap()]);
    expect_exit(&get, 0, "the honest resume lands the file");
    assert_eq!(std::fs::read(&target).unwrap(), BETA);
}

/// The lowercase hex sha256 of a byte slice, for sibling markers in tests.
fn test_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `--json` failures print exactly one NDJSON error line and nothing on stderr:
/// scripts parse stdout, and a plaintext echo would leak into their pipelines.
#[test]
fn json_failure_prints_ndjson_only() {
    let srv = server(Scenario::Atomic);
    let url = format!("{}1.14.0/missing.bin", dir_url(&srv));

    let mut cmd = Command::new(NXR);
    cmd.args(["--json", "get", "--retry", "1", &url, "-o", "x.bin"])
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD")
        .env_remove("NXR_CA_BUNDLE");
    let out = cmd.output().expect("nxr binary runs");
    assert_eq!(code(&out), 1, "the storage 404 is data");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.len(), 0, "stderr must stay empty: {stderr}");
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("one JSON line");
    assert_eq!(parsed["event"], "error");
    assert_eq!(parsed["code"], 1);
    assert!(parsed["hint"].is_string());
}

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

/// retry-after-503: a 5xx that names its pause is waited out and the upload lands.
#[test]
fn retry_after_503_up_recovers() {
    let srv = server(Scenario::RetryAfter503 {
        retry_after_secs: 1,
    });
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);

    expect_exit(
        &nxr(&["up", src.path().to_str().unwrap(), &base]),
        0,
        "up through a 503 with Retry-After",
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
        golden("up.ndjson"),
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
        golden("down.ndjson"),
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
        golden("mirror.ndjson"),
        "the mirror ndjson changed: update tests/golden/mirror.ndjson deliberately"
    );
}

/// The retry surface is part of the same contract: a rate-limited up pins the `retrying` event, name, attempt and reason included.
/// The reason carries the request URL, so the test masks the ephemeral port with `PORT` before comparing.
/// The fixture is otherwise byte-exact.
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
        golden("retrying.ndjson"),
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
        golden("doctor.ndjson"),
        "the doctor ndjson changed: update tests/golden/doctor.ndjson deliberately"
    );
}

// ---- golden ndjson: the failure, the deletion and the audit streams ---------

/// A call that fails in JSON mode: the error record is the whole stream, one line.
/// The reason names the request URL, so the ephemeral port is masked with `PORT`
/// before the comparison, exactly as the retrying golden does.
#[test]
fn golden_ndjson_error_holds() {
    let srv = server(Scenario::Atomic);
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("absent.bin");
    let url = format!("{}absent.bin", dir_url(&srv));
    // `-o` is required: without it `--json` refuses before the request goes out,
    // and the fixture would pin the misuse record instead of the 404.
    let out = nxr(&["--json", "get", &url, "-o", target.to_str().unwrap()]);
    expect_exit(&out, 1, "golden error");
    let port = srv.addr().port().to_string();
    let normalized = stdout(&out).replace(&port, "PORT");
    assert_golden(
        &normalized,
        "error.ndjson",
        "the error ndjson changed: update tests/golden/error.ndjson deliberately",
    );
}

/// A refused digest is the populated shape of `failures`: the legacy list keeps
/// the detail, the structured list carries `{code, name}`, pinned byte-exact.
#[test]
fn golden_ndjson_failures_hold() {
    let srv = server(Scenario::ForeignMarker);
    srv.insert("1.14.0/a.zip", ALPHA);
    srv.insert(
        "1.14.0/a.zip.sha256",
        format!("{}  a.zip\n", "0".repeat(64)).as_bytes(),
    );
    let dst = TempDir::new().unwrap();
    let down = nxr(&[
        "--json",
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "a.zip",
    ]);
    expect_exit(&down, 1, "golden failures");
    let port = srv.addr().port().to_string();
    let normalized = stdout(&down).replace(&port, "PORT");
    assert_golden(
        &normalized,
        "failures.ndjson",
        "the failure ndjson changed: update tests/golden/failures.ndjson deliberately",
    );
}

/// `rm` in one stream: the plan first, then the delete, then the rerun.
/// The plan is the default (nothing is deleted without `--yes`), and the rerun
/// re-enumerates the same name, collects a 404 and still exits 0.
/// The three steps are pinned together because together they are the contract.
#[test]
fn golden_ndjson_rm_holds() {
    let srv = server(Scenario::Atomic);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let base = dir_url(&srv);
    let up = nxr(&["up", src.path().to_str().unwrap(), &base]);
    expect_exit(&up, 0, "seeding the rm golden");

    let plan = nxr(&["--json", "rm", &base, "--name", "a.zip"]);
    expect_exit(&plan, 0, "the rm plan");
    let real = nxr(&["--json", "rm", &base, "--name", "a.zip", "--yes"]);
    expect_exit(&real, 0, "the rm delete");
    let again = nxr(&["--json", "rm", &base, "--name", "a.zip", "--yes"]);
    expect_exit(&again, 0, "the rm rerun");
    let combined = format!("{}{}{}", stdout(&plan), stdout(&real), stdout(&again));
    assert_golden(
        &combined,
        "rm.ndjson",
        "the rm ndjson changed: update tests/golden/rm.ndjson deliberately",
    );
}

/// The audit stream: one finding per problem class, then the audit summary.
/// That summary carries the same `event` as a transfer summary, and `kind` is
/// what tells the two apart, which is what this fixture pins.
#[test]
fn golden_ndjson_audit_holds() {
    let srv = server(Scenario::AuditLitter);
    let repo = format!("{}repository/raw-main/1.0.0/", srv.base_url());
    let out = nxr(&["--json", "audit", &repo]);
    expect_exit(&out, 1, "golden audit");
    let port = srv.addr().port().to_string();
    let normalized = stdout(&out).replace(&port, "PORT");
    assert_golden(
        &normalized,
        "audit.ndjson",
        "the audit ndjson changed: update tests/golden/audit.ndjson deliberately",
    );
}

/// Prefixes select whole segments: `orph/` never picks `orphan/...`, the
/// filter runs before any request, and `audited` counts the selected assets.
#[test]
fn audit_prefix_matches_whole_segments() {
    let srv = server(Scenario::AuditLitter);
    let repo = format!("{}repository/raw-main/1.0.0/", srv.base_url());

    let out = nxr(&["--json", "audit", &repo, "--prefix", "orphan/"]);
    expect_exit(&out, 1, "the orphan is a finding under its own prefix");
    let lines = ndjson(&out);
    let findings: Vec<_> = lines
        .iter()
        .filter(|l| l["event"] == "finding")
        .map(|l| l["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(findings, ["orphan/app.bin"]);
    let summary = lines.last().expect("the audit summary");
    assert_eq!(summary["audited"], 1, "the selected set is one asset");
    assert_eq!(summary["findings"], 1);

    // A prefix that is not a whole segment selects nothing: `orph/` is not a
    // parent of `orphan/`, while a naive `starts_with` would say it is.
    let none = nxr(&["--json", "audit", &repo, "--prefix", "orph/"]);
    expect_exit(&none, 0, "no findings under a prefix that matches nothing");
    let summary = ndjson(&none).into_iter().last().expect("the summary");
    assert_eq!(summary["audited"], 0);
    assert_eq!(summary["findings"], 0);
    assert_eq!(
        summary["facts_available"], true,
        "an empty selection does not make a live index unavailable"
    );

    // A prefix without the trailing slash is the same misuse every transfer
    // command refuses: the grammar is one contract, not per-command.
    let loose = nxr(&["audit", &repo, "--prefix", "orph"]);
    expect_exit(&loose, 2, "a prefix without the trailing slash is misuse");
    assert!(
        stderr(&loose).contains("whole segments"),
        "the refusal names the rule: {}",
        stderr(&loose)
    );
}

// ---- golden completions -----------------------------------------------------

/// The completion scripts are part of the contract, pinned byte-exact by fixtures in tests/golden/.
/// A clap tree change (a command, a flag, a help text) deliberately breaks them.
/// Regenerate with `cargo run -p nexus-raw -- complete <shell> > <fixture>` and commit on purpose.
/// The scripts are deterministic: no ports, no timestamps, no environment.
/// A platform-flavored render (an `.exe` in the program name, a path separator) fails here
/// with the first differing lines printed, so the flavor is visible from the CI log alone.
/// The fixture content, normalized: include_str! reads the checkout's bytes,
/// and a windows checkout with core.autocrlf hands golden files over with CRLF.
/// The tests judge content, not the checkout's line-ending convention.
fn golden(name: &str) -> String {
    match name {
        "up.ndjson" => include_str!("golden/up.ndjson"),
        "down.ndjson" => include_str!("golden/down.ndjson"),
        "mirror.ndjson" => include_str!("golden/mirror.ndjson"),
        "retrying.ndjson" => include_str!("golden/retrying.ndjson"),
        "doctor.ndjson" => include_str!("golden/doctor.ndjson"),
        "diff.ndjson" => include_str!("golden/diff.ndjson"),
        "error.ndjson" => include_str!("golden/error.ndjson"),
        "failures.ndjson" => include_str!("golden/failures.ndjson"),
        "rm.ndjson" => include_str!("golden/rm.ndjson"),
        "audit.ndjson" => include_str!("golden/audit.ndjson"),
        _ => unreachable!("the golden table and the fixture name drift together"),
    }
    .replace("\r\n", "\n")
}

fn assert_golden(actual: &str, fixture: &str, what: &str) {
    // include_str! reads the checkout's bytes, and a windows checkout with
    // core.autocrlf hands the fixture over with CRLF: both sides normalize.
    let expected = match fixture {
        "complete-bash.txt" => include_str!("golden/complete-bash.txt").to_owned(),
        "complete-zsh.txt" => include_str!("golden/complete-zsh.txt").to_owned(),
        "complete-fish.txt" => include_str!("golden/complete-fish.txt").to_owned(),
        "complete-powershell.txt" => include_str!("golden/complete-powershell.txt").to_owned(),
        // The ndjson fixtures live in one table (`golden`): a second list here
        // would be a second place to forget when a stream gains a fixture.
        other if other.ends_with(".ndjson") => golden(other),
        _ => unreachable!("the golden table and the fixture name drift together"),
    }
    .replace("\r\n", "\n");
    if actual == expected {
        return;
    }
    let actual_lines: Vec<&str> = actual.lines().collect();
    let expected_lines: Vec<&str> = expected.lines().collect();
    let mut diffs = String::new();
    for (i, (a, e)) in actual_lines
        .iter()
        .zip(expected_lines.iter())
        .enumerate()
        .filter(|(_, (a, e))| a != e)
        .take(6)
    {
        diffs.push_str(&format!("line {}: actual {a:?} != expected {e:?}\n", i + 1));
    }
    if actual_lines.len() != expected_lines.len() {
        diffs.push_str(&format!(
            "line count: actual {} != expected {}\n",
            actual_lines.len(),
            expected_lines.len()
        ));
    }
    if diffs.is_empty() {
        diffs.push_str(&format!(
            "lines are pairwise equal: the difference is at the tail\nactual ends with {:?}, expected ends with {:?}\n",
            actual.get(actual.len().saturating_sub(4)..),
            expected.get(expected.len().saturating_sub(4)..)
        ));
    }
    panic!("{what} changed: update tests/golden/{fixture} deliberately\nplatform flavor:\n{diffs}");
}

#[test]
fn golden_complete_bash_holds() {
    let out = nxr(&["complete", "bash"]);
    expect_exit(&out, 0, "golden complete bash");
    assert_golden(&stdout(&out), "complete-bash.txt", "the bash completion");
}

#[test]
fn golden_complete_zsh_holds() {
    let out = nxr(&["complete", "zsh"]);
    expect_exit(&out, 0, "golden complete zsh");
    assert_golden(&stdout(&out), "complete-zsh.txt", "the zsh completion");
}

#[test]
fn golden_complete_fish_holds() {
    let out = nxr(&["complete", "fish"]);
    expect_exit(&out, 0, "golden complete fish");
    assert_golden(&stdout(&out), "complete-fish.txt", "the fish completion");
}

#[test]
fn golden_complete_powershell_holds() {
    let out = nxr(&["complete", "powershell"]);
    expect_exit(&out, 0, "golden complete powershell");
    assert_golden(
        &stdout(&out),
        "complete-powershell.txt",
        "the powershell completion",
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
        // With --json the NDJSON stream is the only channel: stderr stays empty,
        // and the hint rides the error object below.
        assert!(
            stderr(&up).is_empty(),
            "no plaintext echo beside --json: {}",
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
        let mut failures: Vec<(&str, u64)> = summary["failures"]
            .as_array()
            .expect("failures is a list")
            .iter()
            .map(|f| {
                (
                    f["name"].as_str().expect("a failure name"),
                    f["code"].as_u64().expect("a failure code"),
                )
            })
            .collect();
        failures.sort_unstable();
        assert_eq!(
            failures,
            [("a.zip", 3), ("b.bin", 3)],
            "the structured list carries the transport class: {events:?}"
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
    // The NDJSON stream is the only channel: the hint rides the error object.
    assert!(
        stderr(&guarded).is_empty(),
        "no plaintext echo beside --json: {}",
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

    // Deep probes talk to a server: without a URL they would silently do
    // nothing and still report "all checks passed".
    let deep = nxr(&["doctor", "--deep"]);
    expect_exit(&deep, 2, "--deep requires a URL");
    assert!(
        stderr(&deep).contains("--deep"),
        "the refusal names the flag: {}",
        stderr(&deep)
    );
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
        probe["detail"].as_str().unwrap().contains("HTTP 401"),
        "got: {probe}"
    );
    assert!(
        probe["detail"]
            .as_str()
            .unwrap()
            .contains("any non-5xx means the server is reachable"),
        "the reachability note rides every HEAD fallback line: {probe}"
    );

    let last = lines.last().expect("the error object closes the stream");
    assert_eq!(last["event"], "error", "got: {last}");
    assert_eq!(last["code"], 3, "got: {last}");
    let hint = last["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("pass -u user:pass"),
        "a rejected probe hints at credentials, not the network: {hint}"
    );
    assert!(
        !hint.contains("check the network"),
        "the generic transport hint would mislead here: {hint}"
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

/// `doctor --deep` asks the service API at the origin, not under the repository path.
#[test]
fn doctor_deep_reads_the_service_api_at_the_origin() {
    let srv = server(Scenario::Atomic);
    srv.insert(
        "service/rest/v1/repositories",
        br#"[{"name":"raw-main","format":"raw","type":"hosted"}]"#,
    );
    let url = format!("{}repository/raw-main/1.14.0/", srv.base_url());
    let out = nxr(&["doctor", &url, "--deep"]);
    expect_exit(&out, 0, "deep probes pass");
    let text = stdout(&out);
    assert!(
        text.contains("deep:repo-type"),
        "the probe is reported: {text}"
    );
    assert!(
        !text.contains("deep:repo-type: skipped"),
        "the service API is at the origin, not under the version path: {text}"
    );
}

// ---- rm and point ---------------------------------------------------------

/// Seed the version directory through `up` and install the version document.
/// `names` are what the calling test passes to `rm --name`: the only enumeration a client has.
fn publish_for_rm(srv: &MockNexus, names: &[&str]) -> String {
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
    let mut cmd = vec!["rm".to_string(), base.clone()];
    for n in names {
        cmd.push("--name".to_string());
        cmd.push((*n).to_string());
    }
    let _ = cmd;
    base
}

/// `rm` deletes the enumerated version: names, markers and the version document.
/// A rerun sees only 404s and exits 0.
#[test]
fn rm_removes_whole_version_and_reruns_clean() {
    let srv = server(Scenario::Atomic);
    let base = publish_for_rm(&srv, &["a.zip", "b.bin", "version.json"]);

    let rm = nxr(&[
        "rm",
        "--yes",
        &base,
        "--name",
        "a.zip",
        "--name",
        "b.bin",
        "--name",
        "version.json",
    ]);
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

    let second = nxr(&[
        "rm",
        "--yes",
        &base,
        "--name",
        "a.zip",
        "--name",
        "b.bin",
        "--name",
        "version.json",
    ]);
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
    let base = publish_for_rm(&srv, &["a.zip", "b.bin"]);

    let rm = nxr(&["rm", "--yes", &base, "--name", "a.zip", "--name", "b.bin"]);
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

/// rm plans by default: the plan prints and the mock counts zero DELETEs.
#[test]
fn rm_plans_by_default_and_touches_no_bytes() {
    let srv = server(Scenario::Atomic);
    let base = publish_for_rm(&srv, &["a.zip", "b.bin", "version.json", "ghost.bin"]);

    let dry = nxr(&[
        "rm",
        &base,
        "--name",
        "a.zip",
        "--name",
        "b.bin",
        "--name",
        "version.json",
        "--name",
        "ghost.bin",
    ]);
    expect_exit(&dry, 0, "rm without --yes");
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
    let base = publish_for_rm(&srv, &["a.zip", "b.bin", "version.json"]);

    let rm = nxr(&[
        "--json",
        "rm",
        "--yes",
        &base,
        "--name",
        "a.zip",
        "--name",
        "b.bin",
        "--name",
        "version.json",
    ]);
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
    let second = nxr(&[
        "--json",
        "rm",
        "--yes",
        &base,
        "--name",
        "a.zip",
        "--name",
        "b.bin",
        "--name",
        "version.json",
    ]);
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

/// `point` deletes a channel ref and stays idempotent.
/// The read-only scenario refuses it.
#[test]
fn point_roundtrip_and_readonly() {
    let srv = server(Scenario::Atomic);
    let url = format!("{}stable", root_url(&srv));
    expect_exit(&nxr(&["channel", "set", &url, "1.4.0"]), 0, "channel set");

    let clear = nxr(&["point", &url]);
    expect_exit(&clear, 0, "first clear");
    assert!(stdout(&clear).contains("cleared"), "{}", stdout(&clear));
    assert!(srv.store_get("stable").is_none());

    let again = nxr(&["point", &url]);
    expect_exit(&again, 0, "clearing an absent pointer is exit 0");
    assert!(stdout(&again).contains("absent"), "{}", stdout(&again));

    let ro = server(Scenario::ReadOnly);
    ro.insert("stable", b"1.4.0\n");
    let ro_url = format!("{}stable", root_url(&ro));
    let refused = nxr(&["point", &ro_url]);
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

    let out = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--name",
        "a.zip",
        "--name",
        "manifest.json",
    ]);
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

/// `--prefix` keeps only whole-segment matches on both sides.
#[test]
fn diff_prefix_filters_both_sides() {
    let srv = server(Scenario::Atomic);
    let base = dir_url(&srv);
    let local = TempDir::new().unwrap();
    std::fs::create_dir_all(local.path().join("a")).unwrap();
    std::fs::create_dir_all(local.path().join("b")).unwrap();
    write_file(local.path(), "a/x.bin", ALPHA);
    write_file(local.path(), "b/y.bin", BETA);
    expect_exit(
        &nxr(&["up", local.path().to_str().unwrap(), &base]),
        0,
        "seed the prefix fixture",
    );

    let out = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--name",
        "a/x.bin",
        "--name",
        "b/y.bin",
        "--prefix",
        "a/",
    ]);
    expect_exit(&out, 0, "the filtered diff is clean");
    let text = stdout(&out);
    assert!(text.contains("same a/x.bin"), "{text}");
    assert!(!text.contains("b/y.bin"), "the other prefix is out: {text}");
}

/// `diff` prints one line per entry and changes nothing on either side.
#[test]
fn diff_reports_the_sections_and_writes_nothing() {
    let (srv, local, _scratch) = seed_diff_fixture();
    let before = put_requests(&srv).len();
    let out = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &dir_url(&srv),
        "--name",
        "a.zip",
        "--name",
        "b.zip",
        "--name",
        "c.zip",
        "--name",
        "d.zip",
        "--name",
        "g.zip",
        "--name",
        "manifest.json",
        "--name",
        "zzz.txt",
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
    let (srv, local, _scratch) = seed_diff_fixture();
    let before = put_requests(&srv).len();
    let out = nxr(&[
        "--json",
        "diff",
        local.path().to_str().unwrap(),
        &dir_url(&srv),
        "--name",
        "a.zip",
        "--name",
        "b.zip",
        "--name",
        "c.zip",
        "--name",
        "d.zip",
        "--name",
        "g.zip",
        "--name",
        "manifest.json",
        "--name",
        "zzz.txt",
    ]);
    expect_exit(&out, 1, "the golden diff finds differences");
    assert_eq!(
        stdout(&out),
        golden("diff.ndjson"),
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
    let dst = server(Scenario::Atomic);
    let plan = nxr(&[
        "mirror",
        &dir_url(&src),
        &dir_url(&dst),
        "--dry-run",
        "--name",
        "a.zip",
    ]);
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

#[test]
fn mirror_plan_is_the_dry_run_alias() {
    let src = server(Scenario::Atomic);
    src.insert("1.14.0/a.zip", ALPHA);
    let dst = server(Scenario::Atomic);
    let plan = nxr(&[
        "mirror",
        &dir_url(&src),
        &dir_url(&dst),
        "--plan",
        "--name",
        "a.zip",
    ]);
    expect_exit(&plan, 0, "mirror --plan");
    assert!(
        stdout(&plan).contains("copy a.zip"),
        "got: {}",
        stdout(&plan)
    );
    assert!(
        dst.store_get("1.14.0/a.zip").is_none(),
        "the plan alias writes nothing"
    );
}

// ---- mirror per-side credentials -------------------------------------------

fn seed_mirror_source(srv: &MockNexus) {
    srv.insert("1.14.0/a.zip", ALPHA);
    srv.insert(
        "1.14.0/a.zip.sha256",
        format!("{}  a.zip\n", hex_digest(ALPHA)).as_bytes(),
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
        "--name",
        "a.zip",
    ]);
    expect_exit(&ok, 0, "destination credentials land the copy");
    assert_eq!(dst.store_get("1.14.0/a.zip").unwrap(), ALPHA);

    // Without the flag the destination rejects the writes: nothing lands, nothing lost.
    let dst2 = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });
    let bad = nxr(&["mirror", &dir_url(&src), &dir_url(&dst2), "--name", "a.zip"]);
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
        "--name",
        "a.zip",
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
        "--name",
        "a.zip",
    ]);
    expect_exit(&bad, 3, "a wrong source secret fails the reads");
    assert!(dst2.store_get("1.14.0/a.zip").is_none());
}

/// A shared `-u` authenticates both sides when no per-side override is given.
#[test]
fn mirror_shared_user_authenticates_both_sides() {
    let src = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });
    seed_mirror_source(&src);
    let dst = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });

    let ok = nxr(&[
        "-u",
        "nexus:secret",
        "mirror",
        &dir_url(&src),
        &dir_url(&dst),
        "--name",
        "a.zip",
    ]);
    expect_exit(&ok, 0, "the shared -u authenticates both sides");
    assert_eq!(dst.store_get("1.14.0/a.zip").unwrap(), ALPHA);
}

/// `mirror --names-from` copies exactly the listed names.
#[test]
fn mirror_names_from_copies_only_the_list() {
    let src = server(Scenario::Atomic);
    seed_mirror_source(&src);
    src.insert("1.14.0/b.bin", BETA);
    src.insert("1.14.0/b.bin.sha256", marker_line("b.bin", BETA).as_bytes());
    let dst = server(Scenario::Atomic);
    let list = TempDir::new().unwrap();
    let names = list.path().join("names.txt");
    std::fs::write(&names, "a.zip\n").unwrap();

    let out = nxr(&[
        "mirror",
        &dir_url(&src),
        &dir_url(&dst),
        "--names-from",
        names.to_str().unwrap(),
    ]);
    expect_exit(&out, 0, "the listed name is mirrored");
    assert_eq!(dst.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
    assert!(
        dst.store_get("1.14.0/b.bin").is_none(),
        "an unlisted name is not copied"
    );
}

// ---- mv --------------------------------------------------------------------

/// `mv` pours the version into the destination and then empties the source, markers included.
#[test]
fn mv_moves_and_empties_the_source() {
    let src = server(Scenario::Atomic);
    seed_mirror_source(&src);
    let dst = server(Scenario::Atomic);

    let out = nxr(&[
        "mv",
        "--yes",
        &dir_url(&src),
        &dir_url(&dst),
        "--name",
        "a.zip",
    ]);
    expect_exit(&out, 0, "a converged move is exit 0");

    // The destination holds the whole poured name.
    assert_eq!(dst.store_get("1.14.0/a.zip").unwrap(), ALPHA);
    assert!(dst.store_get("1.14.0/a.zip.sha256").is_some());
    // The source lost the bytes and the marker.
    assert!(src.store_get("1.14.0/a.zip").is_none());
    assert!(src.store_get("1.14.0/a.zip.sha256").is_none());
}

/// `mv` plans by default: both plans print and nothing moves.
#[test]
fn mv_plans_by_default_and_prints_both_plans() {
    let src = server(Scenario::Atomic);
    seed_mirror_source(&src);
    let dst = server(Scenario::Atomic);

    let out = nxr(&["mv", &dir_url(&src), &dir_url(&dst), "--name", "a.zip"]);
    expect_exit(&out, 0, "a plan is exit 0");
    let text = stdout(&out);
    assert!(text.contains("will move:"), "{text}");
    assert!(text.contains("copy a.zip"), "{text}");
    assert!(text.contains("will delete:"), "{text}");
    assert!(text.contains("rm a.zip"), "{text}");

    assert!(dst.store_get("1.14.0/a.zip").is_none(), "nothing moved");
    assert!(src.store_get("1.14.0/a.zip").is_some(), "nothing deleted");
}

/// `mv --json` without `--yes` is one plan object per line: phase headers are
/// human prose and must not leak into the NDJSON stream.
#[test]
fn mv_plan_json_is_pure_ndjson() {
    let src = server(Scenario::Atomic);
    seed_mirror_source(&src);
    let dst = server(Scenario::Atomic);

    let out = nxr(&[
        "--json",
        "mv",
        &dir_url(&src),
        &dir_url(&dst),
        "--name",
        "a.zip",
    ]);
    expect_exit(&out, 0, "a plan is exit 0");
    let lines = ndjson(&out);
    assert!(
        lines.iter().all(|l| l.is_object()),
        "every line is JSON: {lines:?}"
    );
    assert!(lines.iter().any(|l| l["plan"] == "copy"), "{lines:?}");
    assert!(lines.iter().any(|l| l["action"] == "rm"), "{lines:?}");
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

    let out = nxr(&[
        "mv",
        "--yes",
        &dir_url(&src),
        &dir_url(&dst),
        "--name",
        "a.zip",
    ]);
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

    let out = nxr(&[
        "mv",
        "--yes",
        &dir_url(&src),
        &dir_url(&dst),
        "--name",
        "a.zip",
    ]);
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

/// `--json` is one object with the repository array.
/// A repository URL works as the input.
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

/// `service status` prints the alive verdict.
/// `--json` carries the same shape.
#[test]
fn service_status_prints_the_alive_verdict() {
    let srv = server(Scenario::ServiceStatusVersion);
    let out = nxr(&["service", "status", &srv.base_url()]);
    expect_exit(&out, 0, "a live server is a zero");
    let text = stdout(&out);
    assert!(text.contains("alive"), "{text}");
}

/// `service repo` resolves the repository behind any URL inside it.
#[test]
fn service_repo_resolves_from_a_deep_url() {
    let srv = server(Scenario::Atomic);
    let inside = format!("{}repository/raw-main/1.0.0/", srv.base_url());
    let out = nxr(&["service", "repo", &inside, "--json"]);
    expect_exit(&out, 0, "the prefix match resolves");
    let v: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(v["name"], "raw-main");
    assert_eq!(v["format"], "raw");
}

/// `service repo` on a URL the server does not serve is a data refusal.
#[test]
fn service_repo_unknown_url_is_a_data_miss() {
    let srv = server(Scenario::Atomic);
    let ghost = format!("{}repository/ghost/", srv.base_url());
    let out = nxr(&["service", "repo", &ghost]);
    expect_exit(&out, 1, "a miss is a data fact");
}

/// A server-supplied name with control characters is escaped in the human line.
#[test]
fn ls_escapes_a_hostile_server_name() {
    let srv = server(Scenario::Atomic);
    let page = serde_json::json!({"items": [{"path": "bad\nname.bin"}]});
    srv.insert("service/rest/v1/search/assets", page.to_string().as_bytes());
    let repo = format!("{}repository/raw-main/", srv.base_url());
    let out = nxr(&["ls", &repo]);
    expect_exit(&out, 0, "the listing succeeds");
    let text = stdout(&out);
    assert!(
        text.contains("\\n"),
        "the newline is escaped, not printed raw: {text:?}"
    );
}

/// `down --ls --plan` uses the store-backed search enumeration: the plan lists the searched names.
#[test]
fn down_ls_plan_lists_the_search_names() {
    let srv = server(Scenario::SearchLag { lag_ms: 10 });
    srv.insert("repository/raw-main/g/1.0.0/a.zip", ALPHA);
    srv.insert(
        "repository/raw-main/g/1.0.0/a.zip.sha256",
        marker_line("a.zip", ALPHA).as_bytes(),
    );
    // The search lag is armed by a storage PUT.
    let probe = TempDir::new().unwrap();
    std::fs::write(probe.path().join("p"), b"probe").unwrap();
    let put = nxr(&[
        "put",
        &format!("{}repository/raw-main/g/1.0.0/.probe", srv.base_url()),
        "-f",
        probe.path().join("p").to_str().unwrap(),
    ]);
    expect_exit(&put, 0, "the probe arms the search lag");
    std::thread::sleep(std::time::Duration::from_millis(100));

    let version = format!("{}repository/raw-main/g/1.0.0/", srv.base_url());
    let dst = TempDir::new().unwrap();
    let plan = nxr(&[
        "down",
        &version,
        dst.path().to_str().unwrap(),
        "--ls",
        "--plan",
    ]);
    expect_exit(&plan, 0, "the search plan succeeds");
    assert!(
        stdout(&plan).contains("download a.zip"),
        "the plan names the searched asset: {}",
        stdout(&plan)
    );
}

/// `ls --assets` prints flat artifact names and refuses a repository root.
#[test]
fn ls_assets_lists_flat_names_and_refuses_a_repository_root() {
    let srv = server(Scenario::AssetsPagination);
    let version = format!("{}repository/raw-main/v0.9.0-assets/", srv.base_url());
    let out = nxr(&["ls", &version, "--assets"]);
    expect_exit(&out, 0, "the flat listing succeeds");
    let text = stdout(&out);
    assert!(text.contains("asset-000.bin"), "a flat name: {text}");
    assert!(!text.contains(".sha256"), "markers are hidden: {text}");

    let root = nxr(&["ls", &srv.base_url(), "--assets"]);
    expect_exit(&root, 2, "a repository root cannot be an assets base");
}

/// `service assets` walks the search pages: one --json object with every asset.
#[test]
fn service_assets_walks_the_search_pages() {
    let srv = server(Scenario::AssetsPagination);
    let repo = format!("{}repository/raw-main/", srv.base_url());
    let out = nxr(&["service", "assets", &repo, "--json"]);
    expect_exit(&out, 0, "the walk succeeds");
    let v: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    let assets = v["assets"].as_array().expect("assets array");
    assert_eq!(assets.len(), 35, "the mock serves 35 assets");
    assert_eq!(assets[0]["path"], "v0.9.0-assets/asset-000.bin");
}

/// A server-controlled checksum may be multibyte: the human line never byte-slices it.
#[test]
fn service_assets_survives_a_multibyte_checksum() {
    let srv = server(Scenario::Atomic);
    let page = serde_json::json!({
        "items": [{"path": "1.0.0/a.zip", "checksum": {"sha256": "aéééé"}}],
    });
    srv.insert("service/rest/v1/search/assets", page.to_string().as_bytes());
    let repo = format!("{}repository/raw-main/", srv.base_url());
    let out = nxr(&["service", "assets", &repo]);
    expect_exit(&out, 0, "a multibyte checksum must not panic");
    assert!(
        stdout(&out).contains("1.0.0/a.zip"),
        "got: {}",
        stdout(&out)
    );
}

/// `service eula --accept` on an auth-refusing server is an auth failure, not "no gate".
#[test]
fn service_eula_accept_refusal_is_an_auth_error() {
    let srv = server(Scenario::Auth401 {
        user: "nexus".into(),
        pass: "secret".into(),
    });
    let out = nxr(&["service", "eula", &srv.base_url(), "--accept"]);
    expect_exit(&out, 3, "a refused acceptance is a transport/auth class");
    assert!(stderr(&out).contains("hint:"), "got: {}", stderr(&out));
}

/// `service eula` on a server without the gate reports gate:false and exits zero.
#[test]
fn service_eula_without_a_gate_is_a_noop() {
    let srv = server(Scenario::EulaAbsent);
    let out = nxr(&["service", "eula", &srv.base_url(), "--json"]);
    expect_exit(&out, 0, "no gate is a zero");
    let v: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(v["gate"], false);
}

/// `--ls` and explicit names are two enumeration sources: a destructive command refuses the pair.
#[test]
fn rm_ls_with_explicit_names_is_misuse() {
    let srv = server(Scenario::Atomic);
    srv.insert("1.14.0/a.zip", ALPHA);
    srv.insert(
        "1.14.0/a.zip.sha256",
        marker_line("a.zip", ALPHA).as_bytes(),
    );
    srv.insert("1.14.0/b.bin", BETA);

    let out = nxr(&["rm", &dir_url(&srv), "--ls", "--name", "a.zip", "--yes"]);
    expect_exit(&out, 2, "conflicting enumeration sources are misuse");
    assert!(
        stderr(&out).contains("--ls") && stderr(&out).contains("--name"),
        "the refusal names both sources: {}",
        stderr(&out)
    );
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
    assert_eq!(srv.store_get("1.14.0/b.bin").as_deref(), Some(BETA));
}

// ---- prefix ----------------------------------------------------------------

/// `down --prefix` plans exactly the subtree.
/// A bare prefix is misuse.
#[test]
fn down_prefix_flag_filters_the_plan() {
    let srv = server(Scenario::Atomic);
    seed_subtrees_cli(&srv);
    let dst = tempfile::tempdir().unwrap();

    let out = nxr(&[
        "down",
        &dir_url(&srv),
        dst.path().to_str().unwrap(),
        "--name",
        "bom/a.zip",
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
        "--name",
        "bom/a.zip",
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

/// A 400 from a repository-scoped search for an unknown repository prints the repository hint and exits 3.
#[test]
fn ls_unknown_repository_prints_the_repository_hint() {
    let srv = server(Scenario::Search400);
    let out = nxr(&["ls", &format!("{}repository/raw-ghost/", srv.base_url())]);
    expect_exit(&out, 3, "a refused repository search is transport-class");
    // The hint line is public surface: pin it verbatim.
    let hint_line = "hint: the repository is missing on the server or is not a raw repository";
    assert!(
        stderr(&out).lines().any(|l| l == hint_line),
        "{}",
        stderr(&out)
    );
}

/// A directory listing asks the search with `q=<directory>/`: the 300-name directory of a
/// 5539-row repository lists in 10 pages, not the 111 a repository-wide walk costs (and not
/// the 100-page bound a listing used to carry), and the loose match's decoy names stay out.
#[test]
fn ls_scopes_the_search_to_the_directory() {
    let srv = server(Scenario::AssetsPrefixQ);
    let out = nxr(&[
        "ls",
        &format!("{}repository/raw-main/v1.1.0-dir/", srv.base_url()),
    ]);
    expect_exit(
        &out,
        0,
        "the scoped listing of a 300-name directory succeeds",
    );
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 300, "exactly the directory's own names");
    assert!(
        lines.iter().all(|l| l.starts_with("dir-")),
        "no foreign name among the entries"
    );
    let pages = srv
        .requests()
        .iter()
        .filter(|r| r.path == "service/rest/v1/search/assets")
        .count();
    assert_eq!(
        pages, 10,
        "300 directory rows plus 200 decoy rows the loose q drags in, 50 a page"
    );

    // The same directory as flat names (`--assets`), the enumeration `down --ls` consumes.
    let out = nxr(&[
        "ls",
        "--assets",
        &format!("{}repository/raw-main/v1.1.0-dir/", srv.base_url()),
    ]);
    expect_exit(&out, 0, "the artifact listing succeeds");
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 300);
    assert!(
        !lines.iter().any(|l| l.starts_with("decoy-")),
        "the copy under another parent never surfaces"
    );
}

/// The server's `q` is loose (a path substring), the client's `--prefix` is exact: a value
/// without the trailing slash drags the name-continuing sibling in, the measured
/// `bench/flat` against `bench/flat2` case, and the whole-segment prefix drops it again.
#[test]
fn service_assets_loose_q_meets_the_prefix_filter() {
    let srv = server(Scenario::AssetsPrefixQ);
    let repo = format!("{}repository/raw-main/", srv.base_url());
    let out = nxr(&["service", "assets", &repo, "--q", "v1.1.0-dir", "--json"]);
    expect_exit(&out, 0, "the loose walk succeeds");
    let v: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    let assets = v["assets"].as_array().expect("assets array");
    assert_eq!(
        assets.len(),
        504,
        "300 directory rows, 4 sibling rows and 200 decoy rows the loose q returns"
    );
    assert!(
        assets.iter().any(|a| a["path"]
            .as_str()
            .unwrap_or_default()
            .starts_with("v1.1.0-dir2/")),
        "without the slash the name-continuing sibling rides the answer"
    );

    // The prefix filter is the answer: exactly the directory's names, decoys gone.
    let out = nxr(&[
        "service",
        "assets",
        &repo,
        "--q",
        "v1.1.0-dir",
        "--prefix",
        "v1.1.0-dir",
        "--json",
    ]);
    expect_exit(&out, 0, "the filtered walk succeeds");
    let v: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    let assets = v["assets"].as_array().expect("assets array");
    assert_eq!(assets.len(), 300, "the whole-segment prefix is exact");
    assert!(assets.iter().all(|a| a["path"]
        .as_str()
        .unwrap_or_default()
        .starts_with("v1.1.0-dir/")));
}

// ---- diff --via search ------------------------------------------------------

/// The version-directory URL in repository shape: the search walk needs `/repository/<name>/`.
fn repo_dir_url(srv: &MockNexus) -> String {
    format!("{}repository/raw-main/1.14.0/", srv.base_url())
}

/// Seed a store and the matching search page for one version directory:
/// what the probes see and what the search index reports stay the same facts.
fn seed_via_search_fixture(srv: &MockNexus) -> (String, tempfile::TempDir) {
    let local = tempfile::TempDir::new().unwrap();
    write_file(local.path(), "a.zip", ALPHA);
    write_file(local.path(), "b.zip", BETA);
    let base = repo_dir_url(srv);
    expect_exit(
        &nxr(&["up", local.path().to_str().unwrap(), &base]),
        0,
        "seeding the via-search fixture",
    );
    // The search index reports the two uploaded assets with their digests and
    // sizes, the shape a real Nexus serves. Paths are repository-absolute.
    let page = format!(
        r#"{{"continuationToken":null,"items":[
            {{"path":"1.14.0/a.zip","size":{},"checksum":{{"sha256":"{}"}}}},
            {{"path":"1.14.0/b.zip","size":{},"checksum":{{"sha256":"{}"}}}}
        ]}}"#,
        ALPHA.len(),
        hex_digest(ALPHA),
        BETA.len(),
        hex_digest(BETA),
    );
    srv.insert("service/rest/v1/search/assets", page.as_bytes());
    (base, local)
}

/// Same seed: `diff` and `diff --via search --ls` agree on the delta report, JSON normalized.
/// The search path must not fall back to per-name probes: one listing instead.
#[test]
fn via_search_matches_plain_diff_and_needs_one_listing() {
    let srv = server(Scenario::Atomic);
    let (base, local) = seed_via_search_fixture(&srv);

    let plain = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--name",
        "a.zip",
        "--name",
        "b.zip",
    ]);
    expect_exit(&plain, 0, "the converged directory is equal");
    assert_eq!(stdout(&plain), "same a.zip\nsame b.zip\n");

    let searched = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--via",
        "search",
        "--ls",
    ]);
    expect_exit(&searched, 0, "the search-side delta agrees");
    assert_eq!(
        stdout(&searched),
        stdout(&plain),
        "the search facts must read the same verdict as the probes"
    );

    // The JSON shapes agree too, normalized.
    let plain_json = nxr(&[
        "--json",
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--name",
        "a.zip",
        "--name",
        "b.zip",
    ]);
    expect_exit(&plain_json, 0, "the plain json delta");
    let search_json = nxr(&[
        "--json",
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--via",
        "search",
        "--ls",
    ]);
    expect_exit(&search_json, 0, "the search-side json delta");
    assert_eq!(stdout(&search_json), stdout(&plain_json));

    // O(pages), not O(names): the search path fetches the listing once and
    // never HEADs or GETs the two artifacts.
    let before = srv.requests().len();
    let _ = nxr(&[
        "--json",
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--via",
        "search",
        "--ls",
    ]);
    let reqs = srv.requests();
    let storage = reqs
        .iter()
        .skip(before)
        .filter(|r| {
            (r.method == "GET" || r.method == "HEAD")
                && (r.path.ends_with("/a.zip") || r.path.ends_with("/b.zip"))
        })
        .count();
    assert_eq!(
        storage, 0,
        "the via-search diff probes no artifact: the search page carries the facts"
    );
}

/// A digestless search entry can never read as `same`: the missing fact reports as a divergence.
#[test]
fn via_search_digestless_entry_is_not_same() {
    let srv = server(Scenario::Atomic);
    let (base, local) = seed_via_search_fixture(&srv);
    // b.zip loses its checksum line: nothing to verify against anymore.
    let page = format!(
        r#"{{"continuationToken":null,"items":[
            {{"path":"1.14.0/a.zip","size":{},"checksum":{{"sha256":"{}"}}}},
            {{"path":"1.14.0/b.zip","size":{}}}
        ]}}"#,
        ALPHA.len(),
        hex_digest(ALPHA),
        BETA.len(),
    );
    srv.insert("service/rest/v1/search/assets", page.as_bytes());

    let out = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--via",
        "search",
        "--ls",
    ]);
    expect_exit(&out, 1, "an unverifiable name is a difference");
    assert!(
        stdout(&out).contains("diverged b.zip"),
        "the digestless name reports as diverged: {}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains("same a.zip"),
        "the digest-backed name stays same: {}",
        stdout(&out)
    );
}

/// On search-400 the command refuses with the enumeration-family hint and never
/// prints a false clean delta. With `--fallback` it degrades to the probe-based
/// report and says so.
#[test]
fn via_search_on_search_400_refuses_then_falls_back() {
    // A ghost repository: the scenario refuses its repository-scoped search with 400, like a real Nexus.
    let srv = server(Scenario::Search400);
    let local = tempfile::TempDir::new().unwrap();
    write_file(local.path(), "a.zip", ALPHA);
    write_file(local.path(), "b.zip", BETA);
    expect_exit(
        &nxr(&["up", local.path().to_str().unwrap(), &repo_dir_url(&srv)]),
        0,
        "seeding the served repo",
    );
    let ghost = repo_dir_url(&srv).replace("raw-main", "raw-ghost");

    let refused = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &ghost,
        "--via",
        "search",
        "--ls",
    ]);
    expect_exit(&refused, 1, "the honest refusal is the enumeration class");
    let err = stderr(&refused);
    assert!(err.contains("cannot enumerate"), "{err}");
    assert!(err.contains("search API"), "{err}");
    assert!(err.contains("hint:"), "every error carries a hint: {err}");
    assert!(
        !stdout(&refused).contains("same"),
        "no false clean delta on a refused listing: {}",
        stdout(&refused)
    );

    // A served repo whose search endpoint is absent (404) refuses the same way.
    let absent = server(Scenario::AssetsAbsent);
    expect_exit(
        &nxr(&["up", local.path().to_str().unwrap(), &repo_dir_url(&absent)]),
        0,
        "seeding the search-absent repo",
    );
    let refused2 = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &repo_dir_url(&absent),
        "--via",
        "search",
        "--ls",
    ]);
    expect_exit(&refused2, 1, "a search 404 refuses instead of pretending");
    assert!(stderr(&refused2).contains("cannot enumerate"));

    // --fallback degrades explicitly: a warning line, then the probe-based
    // report over explicit names (a dead search cannot name anything anymore).
    let downgraded = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &repo_dir_url(&absent),
        "--via",
        "search",
        "--ls",
        "--fallback",
        "--name",
        "a.zip",
        "--name",
        "b.zip",
    ]);
    expect_exit(
        &downgraded,
        0,
        "the fallback reports the same converged truth",
    );
    let text = stdout(&downgraded);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.contains(&"same a.zip"), "{text}");
    assert!(lines.contains(&"same b.zip"), "{text}");
    assert!(
        stderr(&downgraded).contains("falling back"),
        "the degradation is announced: {}",
        stderr(&downgraded)
    );
}

/// Misuse matrix for the new flags: an unknown source, `--via` without `--ls`,
/// `--fallback` without `--via`.
#[test]
fn via_flag_misuse_is_refused() {
    let srv = server(Scenario::Atomic);
    let base = dir_url(&srv);
    let local = tempfile::TempDir::new().unwrap();
    write_file(local.path(), "a.zip", ALPHA);

    let out = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--via",
        "tea-leaves",
    ]);
    expect_exit(&out, 2, "an unknown --via source is misuse");
    assert!(stderr(&out).contains("unknown --via source"));

    let out = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--via",
        "search",
    ]);
    expect_exit(&out, 2, "--via search without --ls is misuse");
    assert!(stderr(&out).contains("--ls"));

    let out = nxr(&["diff", local.path().to_str().unwrap(), &base, "--fallback"]);
    expect_exit(&out, 2, "--fallback without --via is misuse");
    assert!(stderr(&out).contains("--via"));

    let out = nxr(&[
        "diff",
        local.path().to_str().unwrap(),
        &base,
        "--via",
        "search",
        "--ls",
        "--name",
        "a.zip",
    ]);
    expect_exit(
        &out,
        2,
        "--name fights the search listing as the name source",
    );
    assert!(stderr(&out).contains("--name"));
}

// ---- segmented get ---------------------------------------------------------

/// 4 segments land the same bytes and digest as the single stream, over the slow scenario.
#[test]
fn get_segments_matches_the_single_stream() {
    let srv = server(Scenario::Slow {
        chunk_delay_ms: 1,
        chunk_size: 64,
    });
    let content: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    srv.insert("big.bin", &content);
    let url = format!("{}big.bin", root_url(&srv));

    let single = TempDir::new().unwrap();
    let got = nxr(&[
        "get",
        &url,
        "-o",
        single.path().join("big.bin").to_str().unwrap(),
    ]);
    expect_exit(&got, 0, "single-stream get");
    let want = std::fs::read(single.path().join("big.bin")).unwrap();
    assert_eq!(want, content);

    let seg = TempDir::new().unwrap();
    let target = seg.path().join("big.bin");
    let got = nxr(&[
        "get",
        "--segments",
        "4",
        &url,
        "-o",
        target.to_str().unwrap(),
    ]);
    expect_exit(&got, 0, "segmented get");
    let have = std::fs::read(&target).unwrap();
    assert_eq!(have, content, "byte-exact against the seeded payload");
    assert_eq!(
        Sha256::digest(&have),
        Sha256::digest(&want),
        "same digest either way"
    );
    // The staging area only outlives a failed run.
    assert!(!seg.path().join("big.bin.segments").exists());
    // The fan-out went over ranged GETs.
    assert!(
        srv.requests()
            .iter()
            .any(|r| r.method == "GET" && r.outcome == Outcome::Status(206)),
        "segments must ride Range requests: {:?}",
        srv.requests()
    );
}

/// A server that ignores Range degrades to the single stream: the summary line is the plain get's.
#[test]
fn get_segments_falls_back_when_range_is_ignored() {
    let srv = server(Scenario::IgnoreRange);
    let content: Vec<u8> = (0..50_000u32).map(|i| (i % 199) as u8).collect();
    srv.insert("big.bin", &content);
    let url = format!("{}big.bin", root_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("big.bin");
    let got = nxr(&[
        "get",
        "--segments",
        "4",
        &url,
        "-o",
        target.to_str().unwrap(),
    ]);
    expect_exit(&got, 0, "fallback get");
    assert_eq!(std::fs::read(&target).unwrap(), content);
    // No ranged answer ever came back: the server answered every GET with 200.
    assert!(
        !srv.requests()
            .iter()
            .any(|r| r.method == "GET" && r.outcome == Outcome::Status(206)),
        "an ignore-range server never answers 206: {:?}",
        srv.requests()
    );
    assert!(!out.path().join("big.bin.segments").exists());
}

/// An object smaller than the segment count still lands byte-exact.
#[test]
fn get_segments_degrades_on_a_tiny_object() {
    let srv = server(Scenario::Atomic);
    srv.insert("tiny.bin", b"012345");
    let url = format!("{}tiny.bin", root_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("tiny.bin");
    let got = nxr(&[
        "get",
        "--segments",
        "16",
        &url,
        "-o",
        target.to_str().unwrap(),
    ]);
    expect_exit(&got, 0, "tiny segmented get");
    assert_eq!(std::fs::read(&target).unwrap(), b"012345");
}

/// The segment count is bounded and an empty object still completes: the
/// split of zero bytes has no span to compute.
#[test]
fn get_segments_bounds_and_empty_object() {
    let srv = server(Scenario::Atomic);
    srv.insert("empty.bin", b"");
    let url = format!("{}empty.bin", root_url(&srv));

    for n in ["0", "1", "65"] {
        let out = TempDir::new().unwrap();
        let got = nxr(&[
            "get",
            "--segments",
            n,
            &url,
            "-o",
            out.path().join("empty.bin").to_str().unwrap(),
        ]);
        expect_exit(&got, 2, "a segment count outside 2..=64 is misuse");
        assert!(
            stderr(&got).contains("between 2 and 64"),
            "the refusal names the bound: {}",
            stderr(&got)
        );
    }

    let out = TempDir::new().unwrap();
    let target = out.path().join("empty.bin");
    let got = nxr(&[
        "get",
        "--segments",
        "4",
        &url,
        "-o",
        target.to_str().unwrap(),
    ]);
    expect_exit(&got, 0, "an empty object has no span but must still land");
    assert_eq!(std::fs::read(&target).unwrap(), b"");
}

/// `--segments` with a stdout target is a misuse, and stdout streaming stays single-stream.
#[test]
fn get_segments_with_stdout_is_misuse() {
    let srv = server(Scenario::Atomic);
    srv.insert("big.bin", b"payload");
    let url = format!("{}big.bin", root_url(&srv));

    let got = nxr(&["get", "--segments", "4", &url]);
    expect_exit(&got, 2, "segments with stdout target");
    assert!(stderr(&got).contains("--segments"), "{}", stderr(&got));

    // A stdout get carries no segments: the body arrives raw through one 200.
    let got = nxr(&["get", &url]);
    expect_exit(&got, 0, "plain stdout get");
    assert_eq!(stdout(&got), "payload");
    assert!(srv
        .requests()
        .iter()
        .any(|r| r.method == "GET" && r.path == "big.bin" && r.outcome == Outcome::Status(200)));
}

/// A failing segment fails the whole get: no partial file under the output name.
#[test]
fn get_segments_failure_leaves_no_output() {
    let srv = server(Scenario::DropConnection);
    let content: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    srv.insert("big.bin", &content);
    let url = format!("{}big.bin", root_url(&srv));

    let out = TempDir::new().unwrap();
    let target = out.path().join("big.bin");
    // DropConnection resets the first request per path: with retry 1 the first segment dies.
    let got = nxr(&[
        "get",
        "--segments",
        "4",
        "--retry",
        "1",
        &url,
        "-o",
        target.to_str().unwrap(),
    ]);
    expect_exit(&got, 3, "segment failure is transport");
    assert!(!target.exists(), "nothing partial is renamed");
    assert!(stderr(&got).contains("transport"), "{}", stderr(&got));
}

/// up --if-changed twice: the second run PUTs nothing, a mutated file re-PUTs, a markerless remote name re-PUTs.
#[test]
fn up_if_changed_skips_verified_and_reups_the_rest() {
    let srv = server(Scenario::Atomic);
    let base = dir_url(&srv);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    write_file(src.path(), "b.bin", BETA);

    // The first run is a plain full upload.
    let first = nxr(&["up", "--if-changed", src.path().to_str().unwrap(), &base]);
    expect_exit(&first, 0, "the first if-changed run uploads everything");
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));

    // The second run: every name has a verified remote marker, nothing is PUT.
    let puts_before = srv.put_count("1.14.0/a.zip");
    let second = nxr(&["up", "--if-changed", src.path().to_str().unwrap(), &base]);
    expect_exit(&second, 0, "the converged rerun is a clean skip");
    assert!(
        stdout(&second).contains("uploaded 0, downloaded 0, skipped 2"),
        "the rerun skips both names: {}",
        stdout(&second)
    );
    assert_eq!(
        srv.put_count("1.14.0/a.zip"),
        puts_before,
        "a converged rerun writes nothing"
    );

    // One local file mutates: exactly that name re-PUTs (bytes then marker).
    // The stale sibling goes with it: a marker that no longer matches the bytes
    // is a broken local copy, which `up` refuses no matter the flag.
    write_file(src.path(), "b.bin", GAMMA);
    std::fs::remove_file(src.path().join("b.bin.sha256")).unwrap();
    let puts_b_before = srv.put_count("1.14.0/b.bin");
    let puts_marker_before = srv.put_count("1.14.0/b.bin.sha256");
    let puts_a_before = srv.put_count("1.14.0/a.zip");
    let third = nxr(&["up", "--if-changed", src.path().to_str().unwrap(), &base]);
    expect_exit(&third, 0, "the mutated name re-uploads");
    assert_eq!(
        srv.put_count("1.14.0/b.bin"),
        puts_b_before + 1,
        "the change is exactly one bytes PUT for b.bin"
    );
    assert_eq!(
        srv.put_count("1.14.0/b.bin.sha256"),
        puts_marker_before + 1,
        "and exactly one marker PUT beside it"
    );
    assert_eq!(
        srv.put_count("1.14.0/a.zip"),
        puts_a_before,
        "the unchanged name never re-uploads"
    );
    assert_eq!(srv.store_get("1.14.0/b.bin").as_deref(), Some(GAMMA));
}

/// `up --if-changed --plan` uses the if-changed classification: a divergent remote is an upload, not a refusal.
#[test]
fn up_if_changed_plan_matches_the_if_changed_run() {
    let srv = server(Scenario::Atomic);
    let base = dir_url(&srv);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);
    let first = nxr(&["up", src.path().to_str().unwrap(), &base]);
    expect_exit(&first, 0, "the seed upload lands");

    write_file(src.path(), "a.zip", BETA);
    std::fs::remove_file(src.path().join("a.zip.sha256")).unwrap();

    // The strict plan refuses the divergence.
    let strict = nxr(&["up", "--plan", src.path().to_str().unwrap(), &base]);
    expect_exit(&strict, 1, "a strict plan reports the mismatch");

    // The if-changed plan lists the upload and exits 0.
    let planned = nxr(&[
        "up",
        "--if-changed",
        "--plan",
        src.path().to_str().unwrap(),
        &base,
    ]);
    expect_exit(&planned, 0, "the if-changed plan accepts the divergence");
    assert!(
        stdout(&planned).contains("upload a.zip"),
        "the plan names the upload: {}",
        stdout(&planned)
    );
    assert_eq!(
        srv.store_get("1.14.0/a.zip").as_deref(),
        Some(ALPHA),
        "a plan writes nothing"
    );
}

/// up --if-changed over a markerless remote copy: no evidence, the name re-PUTs (the repair path).
#[test]
fn up_if_changed_reups_when_the_remote_marker_is_missing() {
    let srv = server(Scenario::Atomic);
    let base = dir_url(&srv);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);

    // Seed the server with bytes but no marker: `up --no-sha` does exactly that.
    let seed = nxr(&["up", "--no-sha", src.path().to_str().unwrap(), &base]);
    expect_exit(&seed, 0, "the seed run leaves the remote markerless");
    assert!(srv.store_get("1.14.0/a.zip.sha256").is_none());

    // --if-changed cannot verify the remote copy, so it re-PUTs and repairs the marker.
    let run = nxr(&["up", "--if-changed", src.path().to_str().unwrap(), &base]);
    expect_exit(&run, 0, "the markerless name is re-uploaded, not skipped");
    assert_eq!(
        srv.store_get("1.14.0/a.zip.sha256").as_deref(),
        Some(marker_line("a.zip", ALPHA).as_bytes()),
        "the run leaves the marker present"
    );
    assert_eq!(srv.store_get("1.14.0/a.zip").as_deref(), Some(ALPHA));
}

// ---- search lag -----------------------------------------------------------

/// `up --wait-index` holds the run until the lagging search index surfaces every
/// uploaded name.
/// The default run without the flag answers before the index does.
#[test]
fn up_wait_index_survives_the_search_lag() {
    let srv = server(Scenario::SearchLag { lag_ms: 1500 });
    // A group-level base: the search visibility check needs the real-world
    // `repository/<repo>/…` store layout the root-style base does not have.
    let base = format!("{}repository/raw-main/g/", srv.base_url());
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);

    let run = nxr(&[
        "up",
        "--wait-index",
        "--index-timeout",
        "10",
        src.path().to_str().unwrap(),
        &base,
        "--json",
    ]);
    expect_exit(&run, 0, "the wait ends with every name visible");
    let lines = ndjson(&run);
    let wait = lines
        .iter()
        .find_map(|l| l.get("search_wait"))
        .expect("a search_wait event is emitted");
    assert_eq!(wait["visible"], 1, "the name surfaces within the budget");
    assert_eq!(wait["total"], 1);
    assert_eq!(wait["timeout"], false, "the catch-up beat the budget");
}

/// A search that never catches up degrades `up --wait-index` to a warning line
/// and still exits zero: the upload itself has succeeded.
#[test]
fn up_wait_index_timeout_is_a_warning_not_a_failure() {
    // The lag only arms on the first PUT, so this run waits the full budget
    // against an index that catches up 1500ms after that PUT and then serves
    // the name: a passing wait. To pin the timeout branch without a slow test,
    // the budget is shorter than the lag.
    let srv = server(Scenario::SearchLag { lag_ms: 10_000 });
    let base = dir_url(&srv);
    let src = TempDir::new().unwrap();
    write_file(src.path(), "a.zip", ALPHA);

    let run = nxr(&[
        "up",
        "--wait-index",
        "--index-timeout",
        "1",
        src.path().to_str().unwrap(),
        &base,
        "--json",
    ]);
    expect_exit(&run, 0, "a visibility timeout is not a failure");
    let lines = ndjson(&run);
    let wait = lines
        .iter()
        .find_map(|l| l.get("search_wait"))
        .expect("a search_wait event is emitted");
    assert_eq!(wait["visible"], 0);
    assert_eq!(wait["total"], 1);
    assert_eq!(wait["timeout"], true, "the budget expired");
}
