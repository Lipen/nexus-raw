//! The download executor: tmp+hash → rename → tmp marker → rename, orphans by pid (protocol §6.2).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::diff::Action;
use crate::error::Error;
use crate::events::{Dir, Summary};
use crate::model::digest::Digest;
use crate::model::name::ArtifactName;
use crate::model::sibling;
use crate::model::state::{bytes_path, sibling_path};
use crate::transport::client::NexusClient;

const ORPHAN_PREFIX: &str = ".nxr-tmp-";

enum Failure {
    /// The digest diverges from the remote sibling: refuse without overwriting (§5.1).
    Mismatch(String),
    Failed(Error),
}

/// Download the artifacts: the claim is fetched and the diff is done.
///
/// Per-name failures land in `Summary.failed` (digest mismatches become
/// `Error::Mismatch`); the Summary event goes out before the error returns.
pub async fn execute(
    client: Arc<NexusClient>,
    dir: PathBuf,
    version: String,
    actions: Vec<Action>,
    workers: Arc<Semaphore>,
) -> Result<Summary, Error> {
    let mut set = tokio::task::JoinSet::new();
    let mut skipped = 0usize;
    for action in actions {
        match action {
            // Already Complete locally (or the local copy is the only one): nothing to do.
            Action::Skip { name, .. } | Action::Upload { name, .. } => {
                skipped += 1;
                client
                    .progress()
                    .done(name.as_str(), Dir::Down, true, 0, None)
                    .await;
            }
            Action::Download { name, size, digest } => {
                let permit = workers
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| Error::misuse(format!("worker semaphore closed: {e}")))?;
                let client = client.clone();
                let dir = dir.clone();
                let version = version.clone();
                set.spawn(async move {
                    let _permit = permit;
                    match download_one(&client, &dir, &version, &name, size, digest).await {
                        Ok(()) => Ok(name),
                        Err((name, failure)) => Err((name, failure)),
                    }
                });
            }
        }
    }
    let mut summary = Summary {
        skipped,
        ..Summary::default()
    };
    let mut failed: Vec<String> = Vec::new();
    let mut mismatched: Vec<String> = Vec::new();
    let mut first_error: Option<Error> = None;
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(_)) => summary.downloaded += 1,
            Ok(Err((name, Failure::Failed(e)))) => {
                failed.push(name.to_string());
                log::error!("down failed: {e}");
                first_error.get_or_insert(e);
            }
            Ok(Err((name, Failure::Mismatch(detail)))) => {
                mismatched.push(format!("{name}: {detail}"));
            }
            Err(e) => return Err(Error::misuse(format!("task panicked: {e}"))),
        }
    }
    summary.failed = failed.iter().chain(mismatched.iter()).cloned().collect();
    client.progress().summary(&summary);
    if !mismatched.is_empty() {
        return Err(Error::Mismatch {
            name: mismatched.join(", "),
            detail: "remote content diverges from its sha-sibling".to_owned(),
        });
    }
    // Transport failures after retries surface as Transport (exit 3);
    // the failed names are listed in the summary above.
    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(summary)
}

/// GET the bytes into a temp file (hash on the fly) → compare with the sibling →
/// rename → marker into a temp file → rename (§6.2.2).
///
/// A partially fetched name leaves neither bytes nor marker at the destination (§6.2.3).
async fn download_one(
    client: &NexusClient,
    dir: &Path,
    version: &str,
    name: &ArtifactName,
    size_hint: Option<u64>,
    expected: Option<Digest>,
) -> Result<(), (ArtifactName, Failure)> {
    let bytes_url = client.object_url(version, name);
    let tmp = tmp_path(dir, name.as_str(), "bytes");
    let fetched = client
        .download_to_file((name.as_str(), Dir::Down), &bytes_url, &tmp, size_hint)
        .await;
    let (done, actual) = match fetched {
        Ok(v) => v,
        Err(e) => return Err((name.clone(), Failure::Failed(e))),
    };
    if let Some(expected) = &expected {
        if &actual != expected {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err((
                name.clone(),
                Failure::Mismatch(format!(
                    "downloaded digest {actual}, remote sibling {expected}"
                )),
            ));
        }
    }
    let final_bytes = bytes_path(dir, name);
    if let Err(e) = tokio::fs::rename(&tmp, &final_bytes).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err((name.clone(), Failure::Failed(Error::io(&final_bytes, e))));
    }
    let marker = sibling::format_line(name.as_str(), &actual);
    let sib_final = sibling_path(dir, name);
    let sib_tmp = tmp_path(dir, &name.sibling(), "marker");
    let write = async {
        tokio::fs::write(&sib_tmp, marker).await?;
        tokio::fs::rename(&sib_tmp, &sib_final).await
    };
    if let Err(e) = write.await {
        let _ = tokio::fs::remove_file(&sib_tmp).await;
        return Err((name.clone(), Failure::Failed(Error::io(&sib_final, e))));
    }
    client
        .progress()
        .done(name.as_str(), Dir::Down, false, done, size_hint)
        .await;
    Ok(())
}

/// A temp file inside the destination directory: dot prefix + pid + name hash (§6.2.4).
fn tmp_path(dir: &Path, name: &str, kind: &str) -> PathBuf {
    dir.join(format!(
        "{ORPHAN_PREFIX}{}-{:016x}.{kind}",
        std::process::id(),
        fnv64(name.as_bytes())
    ))
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// On start, orphans of dead pids are cleaned; a live process's temps are untouched.
pub fn cleanup_orphans(dir: &Path) -> Result<(), Error> {
    let entries = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let fname = entry.file_name();
        let Some(pid) = orphan_pid(&fname.to_string_lossy()) else {
            continue;
        };
        if pid != std::process::id() && !alive(pid) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
}

fn orphan_pid(fname: &str) -> Option<u32> {
    fname
        .strip_prefix(ORPHAN_PREFIX)?
        .split('-')
        .next()?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(target_os = "linux"))]
fn alive(_pid: u32) -> bool {
    // Without /proc assume the process is alive: conservatively keep foreign temps.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmp_names_are_flat_and_prefixed() {
        let dir = Path::new("/tmp");
        let t = tmp_path(dir, "deep/dir/a.zip", "bytes");
        assert!(t.starts_with(dir));
        assert!(t
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(ORPHAN_PREFIX));
        assert!(t.to_str().unwrap().ends_with(".bytes"));
    }

    #[test]
    fn orphan_pid_parsing() {
        assert_eq!(orphan_pid(".nxr-tmp-42-0abc.bytes"), Some(42));
        assert_eq!(orphan_pid("regular.zip"), None);
        assert_eq!(orphan_pid(".nxr-tmp-x-0.bytes"), None);
    }
}
