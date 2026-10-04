//! End-to-end checks against the real binary: the fail-fast contract and the smoke run.

use std::process::Command;

use mock_nexus::{MockNexus, Scenario};

/// The alternate screen enters with this escape sequence.
/// Fail-fast means the process never prints it.
const ALT_SCREEN: &str = "\x1b[?1049h";

/// Seeds a three-level tree into the mock and publishes the search page it lists from.
/// The store keys carry the `repository/raw-main/` prefix: the repositories
/// document points at that path, and GET/HEAD resolve through it.
fn seed_tree(mock: &MockNexus) {
    for path in [
        "app/core/lib.rs",
        "app/core/util/helpers.py",
        "app/readme.txt",
        "docs/guide/intro.md",
        "README.txt",
    ] {
        mock.insert(
            &format!("repository/raw-main/{path}"),
            format!("content of {path}\n").as_bytes(),
        );
    }
    mock.insert(
        "service/rest/v1/search/assets",
        br#"{"continuationToken":null,"items":[
            {"path":"app/core/lib.rs"},
            {"path":"app/core/util/helpers.py"},
            {"path":"app/readme.txt"},
            {"path":"docs/guide/intro.md"},
            {"path":"README.txt"}]}"#,
    );
}

#[test]
fn a_dead_url_fails_before_any_screen() {
    // Port 1 on loopback: nothing listens, the connection is refused at once.
    let out = Command::new(env!("CARGO_BIN_EXE_nxr-tui"))
        .arg("http://127.0.0.1:1/")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "transport exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error:"), "stderr was: {stderr}");
    assert!(stderr.contains("hint:"), "stderr was: {stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains(ALT_SCREEN),
        "the alternate screen must never open: {stdout:?}"
    );
}

#[test]
fn a_malformed_url_is_misuse() {
    let out = Command::new(env!("CARGO_BIN_EXE_nxr-tui"))
        .arg("not a url")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "misuse exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error:"), "stderr was: {stderr}");
    assert!(stderr.contains("hint:"), "stderr was: {stderr}");
}

#[test]
fn missing_args_are_reported() {
    let out = Command::new(env!("CARGO_BIN_EXE_nxr-tui"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));
}

#[test]
fn smoke_walks_and_downloads_against_a_live_mock() {
    let mock = MockNexus::start(Scenario::Atomic).unwrap();
    seed_tree(&mock);

    // The download lands under the working directory of the child process.
    let work = std::env::temp_dir().join(format!("nxr-tui-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_nxr-tui"))
        .args(["--smoke", &mock.base_url()])
        .current_dir(&work)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The repository listing.
    assert!(stdout.contains("server "), "{stdout}");
    assert!(stdout.contains("raw-main"), "{stdout}");

    // The tree walk: root listing plus two levels of descents.
    assert!(stdout.contains("tree of raw-main"), "{stdout}");
    assert!(stdout.contains("level 1: app/"), "{stdout}");
    assert!(stdout.contains("level 2: app/core/"), "{stdout}");
    assert!(stdout.contains("helpers.py"), "{stdout}");

    // The subtree download of the first root folder.
    assert!(stdout.contains("plan: 3 files"), "{stdout}");
    assert!(stdout.contains("summary: downloaded 3"), "{stdout}");
    assert!(stdout.contains("smoke ok"), "{stdout}");

    // The files are on disk, relative to the repo root.
    for path in [
        "app/core/lib.rs",
        "app/core/util/helpers.py",
        "app/readme.txt",
    ] {
        assert!(
            work.join("nxr-tui-smoke/raw-main").join(path).is_file(),
            "missing download: {path}"
        );
    }
    let _ = std::fs::remove_dir_all(&work);
}
