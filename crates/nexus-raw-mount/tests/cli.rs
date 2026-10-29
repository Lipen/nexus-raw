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

/// A URL carrying a credential is refused by the normalization before the shape
/// check, so the message can never echo the password.
#[test]
fn a_url_with_a_credential_is_refused_without_echoing_it() {
    let mountpoint = tempfile::TempDir::new().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nxr-mount"))
        .arg("https://alice:s3cret@127.0.0.1:1/repository/raw-main/")
        .arg(mountpoint.path())
        .env_clear()
        .output()
        .expect("the mount binary runs");
    assert_eq!(out.status.code(), Some(2), "a credential is misuse");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("userinfo"),
        "the normalization names the cause: {err}"
    );
    assert!(
        !err.contains("s3cret"),
        "the password never reaches the output: {err}"
    );
}
