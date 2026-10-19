//! The download executor: resumable part file → verify → rename → local marker (§5.2).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::AsyncWriteExt;

use crate::error::{Error, DATA_EXIT};
use crate::events::{Dir, Failure, Summary};
use crate::model::digest::Digest;
use crate::model::name::ArtifactName;
use crate::model::sibling;
use crate::model::state::{bytes_path, sibling_path};
use crate::primitive::{assemble, MAX_SEGMENTS};
use crate::sync::diff::Action;
use crate::transport::aimd::WorkerLimit;
use crate::transport::client::NexusClient;
use sha2::Digest as _;

/// The stable part-file prefix: `<prefix><128-bit sha256 hex>` per artifact name.
/// A part survives an interrupted run so a rerun resumes it.
const PART_PREFIX: &str = ".nxr-part-";
/// The segmented staging prefix: the same 128-bit name token, so the directory
/// belongs to exactly one artifact and survives an interrupted run.
const SEGMENT_PREFIX: &str = ".nxr-segments-";
/// The staging manifest: the object size and the segment count the files were
/// cut for. A mismatch discards the staging area instead of assembling a
/// tiling that no longer matches the object.
const SEGMENT_MANIFEST: &str = "manifest";
/// Legacy temp prefix from pre-resume versions.
/// Cleaned up when the owning pid is dead.
const ORPHAN_PREFIX: &str = ".nxr-tmp-";

pub(crate) enum Outcome {
    /// The digest diverges from the remote sibling: refuse without overwriting (§5.1).
    Mismatch(String),
    Failed(Error),
}

/// The 128-bit name token shared by the part file and the segmented staging
/// directory.
///
/// Two distinct names colliding into one part file would interleave writes, so
/// the space stays far beyond birthday reach for any plausible directory.
fn name_token(name: &ArtifactName) -> String {
    let digest = sha2::Sha256::digest(name.encoded().as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// The stable part-file path for a name inside `dir`.
#[must_use]
pub fn part_path(dir: &Path, name: &ArtifactName) -> PathBuf {
    dir.join(format!("{PART_PREFIX}{}", name_token(name)))
}

/// The segmented staging directory of a name inside `dir`.
#[cfg(not(target_arch = "wasm32"))]
fn segment_dir(dir: &Path, name: &ArtifactName) -> PathBuf {
    dir.join(format!("{SEGMENT_PREFIX}{}", name_token(name)))
}

/// Download the artifacts.
///
/// Per-name failures land in `Summary.failed` (digest mismatches become `Error::Mismatch`).
/// The Summary event goes out before the error returns.
/// Part files resume by default.
/// `fresh` starts every name from zero.
/// `segments` splits each artifact into that many concurrent Range spans; the
/// worker budget still caps the network concurrency, so the flag widens one
/// object, never the run.
///
/// # Errors
///
/// Returns the first transport/auth/HTTP failure of the run and [`Error::Mismatch`] on a digest divergence.
pub async fn execute(
    client: Arc<NexusClient>,
    dir: PathBuf,
    dir_url: String,
    actions: Vec<Action>,
    fresh: bool,
    workers: Arc<WorkerLimit>,
    segments: Option<usize>,
) -> Result<Summary, Error> {
    if let Some(n) = segments {
        if !(2..=MAX_SEGMENTS).contains(&n) {
            return Err(Error::misuse(format!(
                "segments must be between 2 and {MAX_SEGMENTS}, got {n}"
            )));
        }
    }
    #[cfg(target_arch = "wasm32")]
    if segments.is_some() {
        return Err(Error::misuse(
            "segmented downloads are not available on this target",
        ));
    }
    // The artifact slot bounds how many segmented artifacts build their span
    // futures at once. It is deliberately separate from the AIMD budget: a
    // slot is not a connection, so holding one cannot starve the spans.
    #[cfg(not(target_arch = "wasm32"))]
    let slots = segments.map(|_| Arc::new(tokio::sync::Semaphore::new(workers.limit().max(1))));
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
                // Segmented downloads take a slot, not a worker permit: their
                // span futures acquire the workers one Range at a time.
                #[cfg(not(target_arch = "wasm32"))]
                if let (Some(n), Some(slots)) = (segments, slots.as_ref()) {
                    let slot = slots
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|e| Error::misuse(format!("segment slot closed: {e}")))?;
                    let client = client.clone();
                    let dir = dir.clone();
                    let dir_url = dir_url.clone();
                    let workers = workers.clone();
                    set.spawn(async move {
                        let result = download_segmented(
                            &client,
                            &dir,
                            &dir_url,
                            &name,
                            size,
                            digest,
                            fresh,
                            n,
                            workers.clone(),
                            true,
                        )
                        .await;
                        drop(slot);
                        settle(result, name, &workers)
                    });
                    continue;
                }
                // The AIMD budget admits the task under the current limit (§7):
                // the permit settles with the task's outcome when it finishes.
                let permit = workers.acquire().await.map_err(Error::misuse)?;
                let client = client.clone();
                let dir = dir.clone();
                let dir_url = dir_url.clone();
                let workers = workers.clone();
                set.spawn(async move {
                    let result =
                        download_one(&client, &dir, &dir_url, &name, size, digest, fresh, true)
                            .await;
                    // The permit is dropped first, so a decrease that landed
                    // mid-task reclaims it in `release_*` as owed.
                    drop(permit);
                    settle(result, name, &workers)
                });
            }
        }
    }
    let mut summary = Summary {
        skipped,
        ..Summary::default()
    };
    let mut failed: Vec<(String, Failure)> = Vec::new();
    let mut mismatched: Vec<(String, Failure)> = Vec::new();
    let mut first_error: Option<Error> = None;
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(_)) => summary.downloaded += 1,
            Ok(Err((name, Outcome::Failed(e)))) => {
                failed.push((
                    name.to_string(),
                    Failure {
                        name: name.to_string(),
                        code: e.exit_code(),
                    },
                ));
                log::error!("down failed: {e}");
                first_error.get_or_insert(e);
            }
            Ok(Err((name, Outcome::Mismatch(detail)))) => {
                mismatched.push((
                    format!("{name}: {detail}"),
                    Failure {
                        name: name.to_string(),
                        code: DATA_EXIT,
                    },
                ));
            }
            Err(e) => return Err(Error::misuse(format!("task panicked: {e}"))),
        }
    }
    // Both lists follow the same order: the failed names, then the mismatches.
    summary.failed = failed
        .iter()
        .map(|(legacy, _)| legacy.clone())
        .chain(mismatched.iter().map(|(legacy, _)| legacy.clone()))
        .collect();
    summary.failures = failed
        .iter()
        .map(|(_, failure)| failure.clone())
        .chain(mismatched.iter().map(|(_, failure)| failure.clone()))
        .collect();
    client.progress().summary(&summary);
    if !mismatched.is_empty() {
        return Err(Error::Mismatch {
            name: mismatched
                .iter()
                .map(|(legacy, _)| legacy.as_str())
                .collect::<Vec<_>>()
                .join(", "),
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
/// `emit_done == false` hands the settle event to the caller: the mirror reports one done line for the whole copy, not one per phase.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download_one(
    client: &NexusClient,
    dir: &Path,
    dir_url: &str,
    name: &ArtifactName,
    size_hint: Option<u64>,
    expected: Option<Digest>,
    fresh: bool,
    emit_done: bool,
) -> Result<(), (ArtifactName, Outcome)> {
    let bytes_url = client.object_url(dir_url, name);
    let part = part_path(dir, name);
    // Resuming means the pre-existing part content joins the digest: a stale part (the remote object changed between runs) would poison the check.
    // Without a remote sibling there is nothing to verify against, so markerless names always download from zero (§5.1).
    // `resume` is the intent, and it stays true even when the part does not
    // exist yet: an attempt that cuts mid-body creates the part, and the
    // internal retry must resume it. The client refuses a symlink at the path
    // on its own, so the read side is safe.
    let resume = expected.is_some() && !fresh;
    // The stale-part verdict needs to know whether a regular non-empty part
    // was there before this call.
    let resumed =
        resume && std::fs::symlink_metadata(&part).is_ok_and(|m| m.is_file() && m.len() > 0);
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
        Err(e) => return Err((name.clone(), Outcome::Failed(e))),
    };
    if let Some(expected) = &expected {
        if &actual != expected {
            // The part content diverges from the marker: never resume it later.
            let _ = tokio::fs::remove_file(&part).await;
            if resumed {
                // A stale part, not a diverging server: one clean restart decides.
                // The fresh attempt is digest-guarded as usual.
                log::warn!("{name}: part file was stale, restarting from zero");
                let expected = Some(expected.clone());
                return Box::pin(download_one(
                    client, dir, dir_url, name, size_hint, expected, true, emit_done,
                ))
                .await;
            }
            return Err((
                name.clone(),
                Outcome::Mismatch(format!(
                    "downloaded digest {actual}, remote sibling {expected}"
                )),
            ));
        }
    }
    finish_download(client, dir, name, &part, actual, done, size_hint, emit_done).await
}

/// Rename the verified part into place and write its local marker.
///
/// The bytes are already digest-checked: a destination-side failure (a symlink
/// in the way, a permission) keeps the part for the rerun instead of
/// discarding a complete transfer.
#[allow(clippy::too_many_arguments)]
async fn finish_download(
    client: &NexusClient,
    dir: &Path,
    name: &ArtifactName,
    part: &Path,
    actual: Digest,
    done: u64,
    total: Option<u64>,
    emit_done: bool,
) -> Result<(), (ArtifactName, Outcome)> {
    let final_bytes = bytes_path(dir, name);
    if let Some(parent) = final_bytes.parent() {
        if let Err(e) = ensure_symlink_free(parent, dir) {
            return Err((name.clone(), Outcome::Failed(e)));
        }
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return Err((name.clone(), Outcome::Failed(Error::io(parent, e))));
        }
    }
    if let Err(e) = tokio::fs::rename(part, &final_bytes).await {
        return Err((name.clone(), Outcome::Failed(Error::io(&final_bytes, e))));
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
        return Err((name.clone(), Outcome::Failed(Error::io(&sib_final, e))));
    }
    if emit_done {
        client
            .progress()
            .done(name.as_str(), Dir::Down, false, done, total)
            .await;
    }
    Ok(())
}

/// Settle one artifact's outcome into the worker budget.
fn settle(
    result: Result<(), (ArtifactName, Outcome)>,
    name: ArtifactName,
    workers: &WorkerLimit,
) -> Result<ArtifactName, (ArtifactName, Outcome)> {
    match result {
        Ok(()) => {
            workers.release_success();
            Ok(name)
        }
        Err(e) => {
            workers.release_failure();
            Err(e)
        }
    }
}

/// The split of `total` into `segments` contiguous closed ranges.
#[cfg(not(target_arch = "wasm32"))]
fn split_bounds(total: u64, segments: usize) -> Vec<(u64, u64)> {
    let base = total / segments as u64;
    let rem = total % segments as u64;
    let mut bounds = Vec::with_capacity(segments);
    let mut start = 0;
    for i in 0..segments {
        let len = base + u64::from(i < rem as usize);
        bounds.push((start, start + len - 1));
        start += len;
    }
    debug_assert_eq!(start, total, "the spans must tile the object exactly");
    bounds
}

/// The staging manifest, when it names exactly this object and split.
#[cfg(not(target_arch = "wasm32"))]
async fn read_manifest(staging: &Path) -> Option<(u64, usize)> {
    let text = tokio::fs::read_to_string(staging.join(SEGMENT_MANIFEST))
        .await
        .ok()?;
    let mut words = text.split_whitespace();
    let total = words.next()?.parse().ok()?;
    let segments = words.next()?.parse().ok()?;
    words.next().is_none().then_some((total, segments))
}

#[cfg(not(target_arch = "wasm32"))]
async fn write_manifest(staging: &Path, total: u64, segments: usize) -> Result<(), Error> {
    let path = staging.join(SEGMENT_MANIFEST);
    let mut f = crate::transport::client::write_options(false)
        .open(&path)
        .await
        .map_err(|e| Error::io(&path, e))?;
    f.write_all(format!("{total} {segments}\n").as_bytes())
        .await
        .map_err(|e| Error::io(&path, e))?;
    f.flush().await.map_err(|e| Error::io(&path, e))
}

#[cfg(not(target_arch = "wasm32"))]
async fn segment_complete(path: &Path, span: u64) -> bool {
    tokio::fs::symlink_metadata(path)
        .await
        .is_ok_and(|m| m.is_file() && m.len() == span)
}

/// The plain single-stream transfer, under one worker permit.
///
/// The segmented path falls back here when the object is too small to split,
/// when its size is unknown, or when the server ignores Range.
#[allow(clippy::too_many_arguments)]
#[cfg(not(target_arch = "wasm32"))]
async fn single_stream(
    client: &NexusClient,
    dir: &Path,
    dir_url: &str,
    name: &ArtifactName,
    size_hint: Option<u64>,
    expected: Option<Digest>,
    fresh: bool,
    workers: Arc<WorkerLimit>,
    emit_done: bool,
) -> Result<(), (ArtifactName, Outcome)> {
    let permit = workers
        .acquire()
        .await
        .map_err(|e| (name.clone(), Outcome::Failed(Error::misuse(e))))?;
    let result = download_one(
        client, dir, dir_url, name, size_hint, expected, fresh, emit_done,
    )
    .await;
    drop(permit);
    result
}

/// GET one artifact through `segments` concurrent Range spans.
///
/// Each span streams into its own file under a stable hidden staging
/// directory. A completed span is kept across runs, so a rerun fetches only the
/// spans that did not finish. The spans assemble positionally into the stable
/// part file, one pass hashes it, and the marker check decides the rename,
/// exactly like the single-stream path.
#[allow(clippy::too_many_arguments)]
#[cfg(not(target_arch = "wasm32"))]
async fn download_segmented(
    client: &NexusClient,
    dir: &Path,
    dir_url: &str,
    name: &ArtifactName,
    size_hint: Option<u64>,
    expected: Option<Digest>,
    fresh: bool,
    segments: usize,
    workers: Arc<WorkerLimit>,
    emit_done: bool,
) -> Result<(), (ArtifactName, Outcome)> {
    let bytes_url = client.object_url(dir_url, name);
    let total = match size_hint {
        Some(total) => total,
        None => match client.head_info(&bytes_url).await {
            Ok(info) if (200..300).contains(&info.status) => match info.size {
                Some(total) => total,
                None => {
                    return single_stream(
                        client, dir, dir_url, name, None, expected, fresh, workers, emit_done,
                    )
                    .await;
                }
            },
            Ok(info) => {
                return Err((
                    name.clone(),
                    Outcome::Failed(Error::Http {
                        status: info.status,
                        url: bytes_url,
                    }),
                ));
            }
            Err(e) => return Err((name.clone(), Outcome::Failed(e))),
        },
    };
    // A zero-byte object, or a split that cannot give every span a byte: the
    // plain path does the job without a staging area.
    if total == 0 || total < segments as u64 {
        return single_stream(
            client,
            dir,
            dir_url,
            name,
            Some(total),
            expected,
            fresh,
            workers,
            emit_done,
        )
        .await;
    }
    let part = part_path(dir, name);
    let staging = segment_dir(dir, name);
    let bounds = split_bounds(total, segments);
    // The staging path is predictable beside the destination. A planted
    // symlink or a non-directory must never be followed as resume state.
    let sane = tokio::fs::symlink_metadata(&staging)
        .await
        .is_ok_and(|meta| meta.is_dir());
    if !sane {
        let _ = tokio::fs::remove_file(&staging).await;
    }
    if fresh || !sane || read_manifest(&staging).await != Some((total, segments)) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|e| (name.clone(), Outcome::Failed(Error::io(&staging, e))))?;
        write_manifest(&staging, total, segments)
            .await
            .map_err(|e| (name.clone(), Outcome::Failed(e)))?;
    }
    let fetches: Vec<_> = bounds
        .iter()
        .enumerate()
        .map(|(i, &(start, end_incl))| {
            let file = staging.join(format!("segment-{i:04}.part"));
            let bytes_url = bytes_url.clone();
            let name = name.clone();
            let workers = workers.clone();
            async move {
                let span = end_incl - start + 1;
                if segment_complete(&file, span).await {
                    return Ok::<(usize, u64), (usize, Error)>((i, 0));
                }
                let _permit = workers.acquire().await.map_err(|e| (i, Error::misuse(e)))?;
                client
                    .fetch_range(
                        (name.as_str(), Dir::Down),
                        &bytes_url,
                        start,
                        end_incl,
                        &file,
                    )
                    .await
                    .map(|done| (i, done))
                    .map_err(|e| (i, e))
            }
        })
        .collect();
    for staged in futures_util::future::join_all(fetches).await {
        if let Err((_, e)) = staged {
            if matches!(e, Error::RangeIgnored { .. }) {
                // The server answered a span with the whole body: the plain
                // path is the documented degradation, and the staging area is
                // worthless now.
                let _ = tokio::fs::remove_dir_all(&staging).await;
                let _ = tokio::fs::remove_file(&part).await;
                return single_stream(
                    client,
                    dir,
                    dir_url,
                    name,
                    Some(total),
                    expected,
                    true,
                    workers,
                    emit_done,
                )
                .await;
            }
            return Err((name.clone(), Outcome::Failed(e)));
        }
    }
    if let Err(e) = assemble(&bounds, &staging, &part, total).await {
        return Err((name.clone(), Outcome::Failed(e)));
    }
    let actual = match crate::model::digest::sha256_file(&part) {
        Ok(d) => d,
        Err(e) => return Err((name.clone(), Outcome::Failed(Error::io(&part, e)))),
    };
    if let Some(expected) = &expected {
        if &actual != expected {
            let _ = tokio::fs::remove_file(&part).await;
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err((
                name.clone(),
                Outcome::Mismatch(format!(
                    "downloaded digest {actual}, remote sibling {expected}"
                )),
            ));
        }
    }
    finish_download(
        client,
        dir,
        name,
        &part,
        actual,
        total,
        Some(total),
        emit_done,
    )
    .await?;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    Ok(())
}

/// On start, orphans of dead pids from the pre-resume temp scheme are cleaned.
/// Live process temps are untouched.
/// Part files never clean here: they are the resume state of a rerun.
///
/// # Errors
///
/// Returns [`Error::Io`] when `dir` cannot be listed.
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

/// Every existing directory between the destination root and `parent` must be a real
/// directory: `create_dir_all` and `rename` follow intermediate symlinks, so a planted
/// `bom -> ~/.config` would land the artifact outside the destination.
/// The root itself is the caller's choice and stays allowed.
pub(crate) fn ensure_symlink_free(parent: &Path, root: &Path) -> Result<(), Error> {
    let mut cur = parent.to_owned();
    while cur.starts_with(root) && cur != root {
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.is_symlink() => {
                return Err(Error::io(
                    &cur,
                    std::io::Error::other("refusing to write through a symlinked directory"),
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(&cur, e)),
        }
        cur = match cur.parent() {
            Some(p) => p.to_owned(),
            None => break,
        };
    }
    Ok(())
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
