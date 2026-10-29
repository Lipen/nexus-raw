//! The mount binary's startup contract: a URL a listing can never resolve is
//! refused before the filesystem is served, instead of producing an empty tree.

#![cfg(target_os = "linux")]

/// The repository name comes from the URL, so a URL outside
/// `/repository/<name>/...` cannot list anything: the mount exits 2 with the
/// same message every listing carries, and nothing is mounted.
#[test]
fn a_url_outside_a_repository_is_refused_before_mounting() {
    let mountpoint = tempfile::TempDir::new().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nxr-mount"))
        .arg("http://127.0.0.1:1/repo/")
        .arg(mountpoint.path())
        .env_clear()
        .output()
        .expect("the mount binary runs");
    assert_eq!(out.status.code(), Some(2), "a wrong URL shape is misuse");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("must point inside /repository/"),
        "the refusal names the rule: {err}"
    );
    assert!(
        !mountpoint.path().join("1.0.0").exists(),
        "nothing is served"
    );
}
