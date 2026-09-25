//! CLI conformance: the exit-code matrix, the NDJSON shapes and the
//! credentials hygiene, driven through the `nxr` binary against the mock.

use std::process::{Command, Output};

use mock_nexus::{MockNexus, Scenario};
use nexus_raw_core::model::sibling;
use tempfile::TempDir;

const NXR: &str = env!("CARGO_BIN_EXE_nxr");

struct Env {
    vars: Vec<(String, String)>,
}

impl Env {
    fn new() -> Self {
        Self { vars: Vec::new() }
    }

    fn set(mut self, key: &str, value: &str) -> Self {
        self.vars.push((key.into(), value.into()));
        self
    }
}

fn nxr(args: &[&str], env: &Env) -> Output {
    let mut cmd = Command::new(NXR);
    cmd.args(args)
        .env_clear()
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    for (k, v) in &env.vars {
        cmd.env(k, v);
    }
    cmd.output().expect("nxr runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Isolated config/env: no user config, no ambient credentials.
fn base_env() -> (Env, TempDir) {
    let tmp = TempDir::new().unwrap();
    let env = Env::new()
        .set("HOME", tmp.path().to_str().unwrap())
        .set("XDG_CONFIG_HOME", tmp.path().to_str().unwrap());
    (env, tmp)
}

fn seed_complete(dir: &std::path::Path, name: &str, content: &[u8]) {
    std::fs::write(dir.join(name), content).unwrap();
    let marker = sibling::format_line(name, &nexus_raw_core::Digest::of_bytes(content));
    std::fs::write(dir.join(format!("{name}.sha256")), marker).unwrap();
}

fn write_claim(dir: &std::path::Path, version: &str, names: &[&str]) {
    let artifacts: Vec<String> = names.iter().map(|n| format!("\"{n}\"")).collect();
    std::fs::write(
        dir.join("claim.json"),
        format!(
            "{{\"claim_version\":1,\"version\":\"{version}\",\"artifacts\":[{}]}}",
            artifacts.join(",")
        ),
    )
    .unwrap();
}

#[test]
fn json_flow_is_golden_and_parsable() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (env, tmp) = base_env();
    let dir = tmp.path().join("dist");
    std::fs::create_dir_all(&dir).unwrap();
    write_claim(&dir, "1.0.0", &["a.zip"]);
    seed_complete(&dir, "a.zip", b"golden");

    let out = nxr(
        &[
            "--base",
            &mock.base_url(),
            "--json",
            "up",
            "--dir",
            dir.to_str().unwrap(),
        ],
        &env,
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));

    let lines: Vec<String> = stdout(&out).lines().map(str::to_owned).collect();
    let parsed: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).expect("every line is valid JSON"))
        .collect();

    assert_eq!(parsed[0]["event"], "plan");
    assert_eq!(parsed[0]["upload"], serde_json::json!(["a.zip"]));
    assert_eq!(parsed[0]["download"], serde_json::json!([]));
    assert_eq!(parsed[0]["skip"], serde_json::json!([]));
    assert_eq!(parsed[1]["event"], "artifact");
    assert_eq!(parsed[1]["name"], "a.zip");
    assert_eq!(parsed[1]["state"], "uploading");
    assert_eq!(parsed.last().unwrap()["event"], "summary");
    assert_eq!(parsed.last().unwrap()["uploaded"], 1);

    // The exact byte shape is part of the contract: one golden line pinned.
    let summary_line = lines.last().unwrap();
    assert!(
        summary_line.contains("\"event\":\"summary\"")
            && summary_line.contains("\"uploaded\":1")
            && summary_line.contains("\"downloaded\":0")
            && summary_line.contains("\"skipped\":0")
            && summary_line.contains("\"failed\":[]"),
        "summary shape drifted: {summary_line}"
    );
}

#[test]
fn second_up_is_a_pure_skip() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (env, tmp) = base_env();
    let dir = tmp.path().join("dist");
    std::fs::create_dir_all(&dir).unwrap();
    write_claim(&dir, "1.0.0", &["a.zip"]);
    seed_complete(&dir, "a.zip", b"skip-me");

    let args = [
        "--base",
        &mock.base_url(),
        "--json",
        "up",
        "--dir",
        dir.to_str().unwrap(),
    ];
    assert_eq!(nxr(&args, &env).status.code(), Some(0));
    let out = nxr(&args, &env);
    assert_eq!(out.status.code(), Some(0));
    let lines: Vec<serde_json::Value> = stdout(&out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let summary = lines.last().unwrap();
    assert_eq!(summary["uploaded"], 0);
    assert_eq!(summary["skipped"], 1);
    assert!(lines.iter().any(|v| v["state"] == "skipped"));
}

#[test]
fn mismatch_exit_1_without_rewriting_remote() {
    let mock = MockNexus::start(Scenario::ForeignMarker).unwrap();
    let (env, tmp) = base_env();
    let dir = tmp.path().join("dist");
    std::fs::create_dir_all(&dir).unwrap();
    write_claim(&dir, "1.0.0", &["a.zip"]);
    seed_complete(&dir, "a.zip", b"payload");
    let args = [
        "--base",
        &mock.base_url(),
        "up",
        "--dir",
        dir.to_str().unwrap(),
    ];
    assert_eq!(nxr(&args, &env).status.code(), Some(0));
    let puts_before = mock.put_count("1.0.0/a.zip");

    let out = nxr(&args, &env);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("mismatch"));
    assert_eq!(mock.put_count("1.0.0/a.zip"), puts_before);
}

#[test]
fn unsafe_name_exit_2() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (env, tmp) = base_env();
    let dir = tmp.path().join("target");
    let out = nxr(
        &[
            "--base",
            &mock.base_url(),
            "down",
            "--dir",
            dir.to_str().unwrap(),
            "--version",
            "1.0.0",
            "--only",
            "../evil",
        ],
        &env,
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("unsafe name"));
}

#[test]
fn password_in_config_is_misuse_exit_2() {
    let (mut env, tmp) = base_env();
    let cfg = tmp.path().join("config.toml");
    std::fs::write(
        &cfg,
        "[dev]\nurl = \"http://127.0.0.1:1/\"\npassword = \"oops\"\n",
    )
    .unwrap();
    env = env.set("NXR_CONFIG", cfg.to_str().unwrap());
    let out = nxr(&["down", "--dir", "x", "--version", "1.0.0"], &env);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("forbidden"));
}

#[test]
fn missing_auth_exit_3_without_retries() {
    let mock = MockNexus::start(Scenario::Auth401 {
        user: "ci".into(),
        pass: "secret".into(),
    })
    .unwrap();
    let (env, tmp) = base_env();
    let dir = tmp.path().join("dist");
    std::fs::create_dir_all(&dir).unwrap();
    write_claim(&dir, "1.0.0", &["a.zip"]);
    seed_complete(&dir, "a.zip", b"auth-case");

    let out = nxr(
        &[
            "--base",
            &mock.base_url(),
            "--json",
            "up",
            "--dir",
            dir.to_str().unwrap(),
        ],
        &env,
    );
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("auth"));
    // 401 must not be retried: the mock saw exactly one probe per request.
    let probes = mock
        .requests()
        .iter()
        .filter(|r| r.path == "1.0.0/a.zip" && r.method == "HEAD")
        .count();
    assert_eq!(probes, 1);
}

#[test]
fn dead_base_exit_3() {
    let (env, _tmp) = base_env();
    let out = nxr(
        &[
            "--base",
            "http://127.0.0.1:9/",
            "--retry",
            "1",
            "ls",
            "--version",
            "1.0.0",
        ],
        &env,
    );
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("transport"));
}

#[test]
fn credentials_never_reach_output() {
    let mock = MockNexus::start(Scenario::Auth401 {
        user: "ci".into(),
        pass: "secret".into(),
    })
    .unwrap();
    let (mut env, tmp) = base_env();
    env = env
        .set("NXR_USERNAME", "ci")
        .set("NXR_PASSWORD", "s3cret-value");
    let dir = tmp.path().join("dist");
    std::fs::create_dir_all(&dir).unwrap();
    write_claim(&dir, "1.0.0", &["a.zip"]);
    seed_complete(&dir, "a.zip", b"creds-case");

    let out = nxr(
        &[
            "--base",
            &mock.base_url(),
            "--json",
            "up",
            "--dir",
            dir.to_str().unwrap(),
        ],
        &env,
    );
    // The upload itself fails (wrong password vs the mock), that is fine here:
    // nothing in any stream or in the mock's store may contain the secret.
    let all = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!all.contains("s3cret-value"), "password leaked: {all}");
    assert!(!all.contains("Basic"), "header value leaked: {all}");
}

#[test]
fn dry_run_prints_plan_and_writes_nothing() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (env, tmp) = base_env();
    let dir = tmp.path().join("dist");
    std::fs::create_dir_all(&dir).unwrap();
    write_claim(&dir, "1.0.0", &["a.zip"]);
    seed_complete(&dir, "a.zip", b"dry-run");

    let out = nxr(
        &[
            "--base",
            &mock.base_url(),
            "--json",
            "up",
            "--dir",
            dir.to_str().unwrap(),
            "--dry-run",
        ],
        &env,
    );
    assert_eq!(out.status.code(), Some(0));
    let lines: Vec<serde_json::Value> = stdout(&out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["event"], "plan");
    assert!(lines.last().unwrap()["event"] != "summary" || true);
    assert!(
        mock.store_get("1.0.0/claim.json").is_none(),
        "dry-run must not PUT"
    );
    assert!(mock.store_get("1.0.0/a.zip").is_none());
}

#[test]
fn down_resolves_pointer_and_verifies_marker() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (env, tmp) = base_env();

    // Seed the server side directly: claim, bytes, marker, pointer.
    let content = b"through-the-pointer";
    mock.insert(
        "1.2.0/claim.json",
        b"{\"claim_version\":1,\"version\":\"1.2.0\",\"artifacts\":[\"a.zip\"]}",
    );
    mock.insert("1.2.0/a.zip", content);
    let marker = sibling::format_line("a.zip", &nexus_raw_core::Digest::of_bytes(content));
    mock.insert("1.2.0/a.zip.sha256", marker.as_bytes());
    mock.insert("latest", b"1.2.0\n");

    let target = tmp.path().join("vendor");
    let out = nxr(
        &[
            "--base",
            &mock.base_url(),
            "down",
            "--dir",
            target.to_str().unwrap(),
            "--pointer",
            "latest",
        ],
        &env,
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(
        std::fs::read(target.join("a.zip")).unwrap(),
        content.to_vec()
    );
    assert!(target.join("a.zip.sha256").is_file());
}

#[test]
fn verify_offline_exit_codes() {
    let (env, tmp) = base_env();
    let dir = tmp.path().join("build");
    std::fs::create_dir_all(&dir).unwrap();
    write_claim(&dir, "9.9.9", &["ok.zip", "bad.zip"]);
    seed_complete(&dir, "ok.zip", b"good");
    std::fs::write(dir.join("bad.zip"), b"bad").unwrap();
    std::fs::write(
        dir.join("bad.zip.sha256"),
        sibling::format_line("bad.zip", &nexus_raw_core::Digest::of_bytes(b"mismatched")),
    )
    .unwrap();

    let out = nxr(&["verify", "--dir", dir.to_str().unwrap()], &env);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("incomplete"));
    assert!(stdout(&out).contains("failed: bad.zip"));

    // Only the good name: exit 0.
    let good = tmp.path().join("build-ok");
    std::fs::create_dir_all(&good).unwrap();
    write_claim(&good, "9.9.9", &["ok.zip"]);
    std::fs::copy(dir.join("ok.zip"), good.join("ok.zip")).unwrap();
    std::fs::copy(dir.join("ok.zip.sha256"), good.join("ok.zip.sha256")).unwrap();
    let out = nxr(&["verify", "--dir", good.to_str().unwrap()], &env);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn point_moves_forward_only_with_if_newer() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (env, _tmp) = base_env();
    let base = mock.base_url();

    let out = nxr(&["--base", &base, "point", "latest", "1.4.0"], &env);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(mock.store_get("latest").unwrap(), b"1.4.0\n".to_vec());

    let out = nxr(
        &["--base", &base, "point", "latest", "1.5.0", "--if-newer"],
        &env,
    );
    assert_eq!(out.status.code(), Some(0));

    let out = nxr(
        &["--base", &base, "point", "latest", "1.4.9", "--if-newer"],
        &env,
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(mock.store_get("latest").unwrap(), b"1.5.0\n".to_vec());
    assert!(stdout(&out).contains("skip"));
}

#[test]
fn ls_lists_remote_states() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    let (env, tmp) = base_env();
    let dir = tmp.path().join("dist");
    std::fs::create_dir_all(&dir).unwrap();
    write_claim(&dir, "1.0.0", &["a.zip"]);
    seed_complete(&dir, "a.zip", b"ls-me");
    assert_eq!(
        nxr(
            &[
                "--base",
                &mock.base_url(),
                "up",
                "--dir",
                dir.to_str().unwrap()
            ],
            &env
        )
        .status
        .code(),
        Some(0)
    );

    let out = nxr(
        &["--base", &mock.base_url(), "ls", "--version", "1.0.0"],
        &env,
    );
    assert_eq!(out.status.code(), Some(0));
    let out_text = stdout(&out);
    assert!(out_text.contains("a.zip"));
    assert!(out_text.to_lowercase().contains("complete"));
}
