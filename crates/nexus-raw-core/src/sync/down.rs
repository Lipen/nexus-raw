//! The download executor: resumable part file → verify → rename → local marker (§5.2).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;

use crate::error::Error;
use crate::events::{Dir, Summary};
use crate::model::digest::Digest;
use crate::model::name::ArtifactName;
use crate::model::sibling;
use crate::model::state::{bytes_path, sibling_path};
use crate::sync::diff::Action;
use crate::transport::client::NexusClient;
use sha2::Digest as _;

/// The stable part-file prefix: `<prefix><128-bit sha256 hex>` per artifact name.
/// A part survives an interrupted run so a rerun resumes it.
const PART_PREFIX: &str = ".nxr-part-";
/// Legacy temp prefix from pre-resume versions.
/// Cleaned up when the owning pid is dead.
const ORPHAN_PREFIX: &str = ".nxr-tmp-";

enum Failure {
    /// The digest diverges from the remote sibling: refuse without overwriting (§5.1).
    Mismatch(String),
    Failed(Error),
}

/// The stable part-file path for a name inside `dir`.
///
/// The digest is a 128-bit truncation of SHA-256 over the encoded name: two distinct names colliding into one part file would interleave writes, so the space stays far beyond birthday reach for any plausible directory.
pub fn part_path(dir: &Path, name: &ArtifactName) -> PathBuf {
    let digest = sha2::Sha256::digest(name.encoded().as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    dir.join(format!("{PART_PREFIX}{hex}"))
}

/// Download the artifacts.
///
/// Per-name failures land in `Summary.failed` (digest mismatches become `Error::Mismatch`).
/// The Summary event goes out before the error returns.
/// Part files resume by default.
/// `fresh` starts every name from zero.
pub async fn execute(
    client: Arc<NexusClient>,
    dir: PathBuf,
    dir_url: String,
    actions: Vec<Action>,
    fresh: bool,
    workers: Arc<Semaphore>,
) -> Result<Summary, Error> {
    let mut set = tokio::task::JoinSet::new();
    let mut skipped = 0usize;
    for action in actions {
        match action {
            // In a down plan Skip and Upload both mean the local copy is complete: nothing to fetch.
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
                let dir_url = dir_url.clone();
                set.spawn(async move {
                    let _permit = permit;
                    match download_one(&client, &dir, &dir_url, &name, size, digest, fresh).await {
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
    // Transport failures after retries surface as Transport (exit 3).
    // The failed names are listed in the summary above.
    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(summary)
}

/// GET the bytes into the stable part file (hash on the fly, Range resume) → compare with the sibling → rename → write the local marker (§5.2).
///
/// A refused or failed name leaves only the part file: the bytes and the marker never appear at the destination until the digest checked out.
async fn download_one(
    client: &NexusClient,
    dir: &Path,
    dir_url: &str,
    name: &ArtifactName,
    size_hint: Option<u64>,
    expected: Option<Digest>,
    fresh: bool,
) -> Result<(), (ArtifactName, Failure)> {
    let bytes_url = client.object_url(dir_url, name);
    let part = part_path(dir, name);
    // Resuming means the pre-existing part content joins the digest: a stale part (the remote object changed between runs) would poison the check.
    // Without a remote sibling there is nothing to verify against, so markerless names always download from zero (§5.1).
    // The client itself must not resume, not just the self-heal branch below.
    let resume = expected.is_some() && !fresh;
    // symlink_metadata, not metadata: a symlink at the part path must not be read for the prefix nor trusted for the resume decision.
    // The NOFOLLOW write open would refuse it anyway, so fail the same way before any read.
    let resumed = resume
        && std::fs::symlink_metadata(&part)
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false);
    let fetched = client
        .download_resumable(
            (name.as_str(), Dir::Down),
            &bytes_url,
            &part,
            resume,
            size_hint,
        )
        .await;
    let (done, actual) = match fetched {
        Ok(v) => v,
        Err(e) => return Err((name.clone(), Failure::Failed(e))),
    };
    if let Some(expected) = &expected {
        if &actual != expected {
            // The part content diverges from the marker: never resume it later.
            let _ = tokio::fs::remove_file(&part).await;
            if resumed {
                // A stale part, not a diverging server: one clean restart decides.
                // The fresh attempt is digest-guarded as usual.
                log::warn!("{}: part file was stale, restarting from zero", name);
                let expected = Some(expected.clone());
                return Box::pin(download_one(
                    client, dir, dir_url, name, size_hint, expected, true,
                ))
                .await;
            }
            return Err((
                name.clone(),
                Failure::Mismatch(format!(
                    "downloaded digest {actual}, remote sibling {expected}"
                )),
            ));
        }
    }
    let final_bytes = bytes_path(dir, name);
    if let Some(parent) = final_bytes.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            let _ = tokio::fs::remove_file(&part).await;
            return Err((name.clone(), Failure::Failed(Error::io(parent, e))));
        }
    }
    if let Err(e) = tokio::fs::rename(&part, &final_bytes).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err((name.clone(), Failure::Failed(Error::io(&final_bytes, e))));
    }
    let marker = sibling::format_line(name.as_str(), &actual);
    let sib_final = sibling_path(dir, name);
    let marker_write = async {
        let mut f = crate::transport::client::write_options(false)
            .open(&sib_final)
            .await?;
        f.write_all(marker.as_bytes()).await?;
        f.flush().await
    };
    if let Err(e) = marker_write.await {
        return Err((name.clone(), Failure::Failed(Error::io(&sib_final, e))));
    }
    client
        .progress()
        .done(name.as_str(), Dir::Down, false, done, size_hint)
        .await;
    Ok(())
}

/// On start, orphans of dead pids from the pre-resume temp scheme are cleaned.
/// Live process temps are untouched.
/// Part files never clean here: they are the resume state of a rerun.
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
    fn part_paths_are_stable_per_name() {
        let dir = Path::new("/tmp/x");
        let a = ArtifactName::parse("bom/x.json").unwrap();
        assert_eq!(part_path(dir, &a), part_path(dir, &a));
        let b = ArtifactName::parse("bom/y.json").unwrap();
        assert_ne!(part_path(dir, &a), part_path(dir, &b));
        let part = part_path(dir, &a);
        let name = part.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(PART_PREFIX));
        assert!(!name.contains(std::process::id().to_string().as_str()));
    }

    #[test]
    fn orphan_pid_parses_legacy_temps() {
        assert_eq!(
            orphan_pid(".nxr-tmp-1234-abcdef0123456789.bytes"),
            Some(1234)
        );
        assert_eq!(orphan_pid(".nxr-part-0123456789abcdef"), None);
    }
}
