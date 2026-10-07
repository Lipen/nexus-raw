//! The `Nxr` facade: the single entry point for CLI and wrappers.
//!
//! On wasm32 the surface carries the read paths only: `ls`, `head`, manifests,
//! channels, pointers and the service API. Everything that reads or writes the
//! local filesystem, or fans out over spawned tasks, is native-only.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc, Semaphore};

use crate::config::Config;
use crate::error::Error;
#[cfg(not(target_arch = "wasm32"))]
use crate::events::{Dir, Summary};
use crate::events::{Event, Progress};
use crate::layout::{self, Manifest};
#[cfg(not(target_arch = "wasm32"))]
use crate::model::digest::Digest;
use crate::model::name::ArtifactName;
#[cfg(not(target_arch = "wasm32"))]
use crate::model::state::LocalStatus;
#[cfg(not(target_arch = "wasm32"))]
use crate::model::state::RemoteStatus;
use crate::primitive;
use crate::primitive::HeadInfo;
#[cfg(not(target_arch = "wasm32"))]
use crate::primitive::{GetOutcome, ShaSource};
use crate::service;
#[cfg(not(target_arch = "wasm32"))]
use crate::sync::{
    self, delta, down,
    mirror::{self, MirrorAction},
    rm::RmAction,
    up, Action, Delta, Mode,
};
use crate::transport::client::NexusClient;

/// How a directory command (`down`, `diff`) learns which names the storage holds (spec §5.2.1).
#[derive(Debug, Clone)]
pub enum Enumeration {
    /// A parsed manifest (`--manifest`).
    Manifest(Manifest),
    /// Explicit names (`--name`, repeatable).
    Names(Vec<ArtifactName>),
    /// Best-effort search-API traversal (`--ls`).
    Search,
    /// An enumeration narrowed to whole-segment prefixes (the `--prefix` filter).
    /// The filter applies after the inner source resolves and dedups.
    Filtered {
        /// The enumeration the filter applies to.
        inner: Box<Enumeration>,
        /// Prefixes; a name survives when it matches at least one.
        prefixes: Vec<crate::model::name::NamePrefix>,
    },
}

impl Enumeration {
    /// Keep only the names under one of `prefixes` (the `--prefix` filter).
    /// Applied after resolution and dedup: the inner source stays the single place enumeration happens.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when a prefix fails the name grammar.
    pub fn with_prefixes(self, prefixes: &[String]) -> Result<Self, Error> {
        if prefixes.is_empty() {
            return Ok(self);
        }
        let parsed = prefixes
            .iter()
            .map(|p| crate::model::name::NamePrefix::parse(p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::Filtered {
            inner: Box::new(self),
            prefixes: parsed,
        })
    }
}

/// Facade over the transport and the operation executors.
pub struct Nxr {
    client: Arc<NexusClient>,
    workers: Arc<Semaphore>,
    events: Progress,
    /// The normalized directory URL this invocation works on.
    base: String,
}

impl Nxr {
    /// Build the facade from `cfg`, emitting events into `events`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when [`Config::normalized_base`] refuses the base URL or the transport client cannot be built.
    pub fn new(cfg: Config, events: mpsc::UnboundedSender<Event>) -> Result<Self, Error> {
        let progress = Progress::new(events);
        let base = cfg.normalized_base()?;
        let client = Arc::new(NexusClient::new(&cfg, progress.clone())?);
        let workers = client.workers();
        Ok(Self {
            client,
            workers,
            events: progress,
            base,
        })
    }

    /// The normalized base URL of this invocation.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    // ---- L0: primitives -------------------------------------------------

    /// GET a URL (spec §5.1).
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the GET, [`Error::Io`] when writing or renaming `out` fails, and [`Error::Misuse`] when stdout cannot be written.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn get(
        &self,
        url: &str,
        out: Option<PathBuf>,
        cont: bool,
    ) -> Result<GetOutcome, Error> {
        primitive::get(&self.client, url, out, cont).await
    }

    /// PUT a file, optionally with its sha-sibling (spec §5.1).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when `src` cannot be read or hashed, [`Error::Misuse`] when `src` is not a file, and transport, auth or HTTP errors from the uploads.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn put(
        &self,
        url: &str,
        src: &Path,
        sha: bool,
    ) -> Result<(u64, Option<Digest>), Error> {
        primitive::put(&self.client, url, src, sha).await
    }

    /// HEAD a URL (spec §5.1).
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors after the retry loop is exhausted.
    pub async fn head(&self, url: &str) -> Result<HeadInfo, Error> {
        primitive::head(&self.client, url).await
    }

    /// The sha256 of a file or URL (spec §5.1).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when a file source cannot be read and transport, auth or HTTP errors for a URL source.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn sha(&self, src: ShaSource) -> Result<Digest, Error> {
        primitive::sha(&self.client, src).await
    }

    // ---- L1: transfer ----------------------------------------------------

    /// The artifact names of a local directory (plain-mode scan, §5.2).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the directory cannot be listed and [`Error::UnsafeName`] when a file path violates the name grammar.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn scan(&self, dir: &Path) -> Result<Vec<ArtifactName>, Error> {
        sync::scan_dir(dir)
    }

    /// The symmetric plan without transferring anything (`up --plan`).
    ///
    /// # Errors
    ///
    /// Returns the first [`Verdict`](crate::error::Verdict) refusal of the symmetric diff, transport, auth or HTTP errors while probing the remote states, and [`Error::Misuse`] if a classification task panicked.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn diff(
        &self,
        dir: &Path,
        names: Vec<ArtifactName>,
        mode: Mode,
        markers: bool,
    ) -> Result<Vec<Action>, Error> {
        let locals = sync::local_statuses(dir, names).await;
        let remotes = self
            .remote_states_for(locals.iter().map(|(n, _)| n))
            .await?;
        let dir = dir.to_owned();
        tokio::task::spawn_blocking(move || sync::classify(&dir, mode, markers, locals, remotes))
            .await
            .map_err(|e| Error::misuse(format!("task panicked: {e}")))?
            .map_err(Error::from)
    }

    /// The delta report of a local directory against a storage enumeration (`nxr diff`).
    ///
    /// The enumeration source is mandatory, like `down`.
    /// The report is facts only: no refusal aborts it, and nothing is written on either side.
    /// The name set is the union of the local scan and the enumeration.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] and [`Error::UnsafeName`] from the local scan, [`Error::Enumerate`] when the enumeration is empty, and transport, auth or HTTP errors while probing the remote states.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn delta(&self, dir: &Path, enum_src: Enumeration) -> Result<Vec<Delta>, Error> {
        let remote_names = self.resolve_names(enum_src).await?;
        let local_names = sync::scan_dir(dir)?;
        let locals = sync::local_statuses(dir, local_names).await;
        let remotes = self.remote_states_for(&remote_names).await?;
        let d = dir.to_owned();
        tokio::task::spawn_blocking(move || delta::compare(&d, locals, remotes))
            .await
            .map_err(|e| Error::misuse(format!("task panicked: {e}")))
    }

    /// Upload a local directory (§5.2).
    ///
    /// `names` restricts the transfer (manifest mode).
    /// `None` scans the directory.
    /// Markers are written by default.
    /// `gen_markers == false` (`--no-sha`) uploads bytes only.
    /// `claim` names the file that must land before any other byte does (claim-first publishing, §2): it uploads alone, and a failed claim aborts the run without starting the rest.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] and [`Error::UnsafeName`] from the scan, [`Error::Misuse`] when `dir` is not a directory, the scan is empty, or the claim is unknown, and [`Error::Missing`] when a requested name is absent from the scan.
    /// Returns [`Error::Mismatch`] when a local object is broken or as the first [`Verdict`](crate::error::Verdict) refusal of the diff, and the first transport/auth/HTTP failure or [`Error::Io`] from marker generation.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn up(
        &self,
        dir: &Path,
        names: Option<Vec<ArtifactName>>,
        gen_markers: bool,
        claim: Option<ArtifactName>,
    ) -> Result<Summary, Error> {
        if !dir.is_dir() {
            return Err(Error::misuse(format!("not a directory: {}", dir.display())));
        }
        let names = if let Some(list) = names {
            let scanned = self.scan(dir)?;
            let have: std::collections::HashSet<&ArtifactName> = scanned.iter().collect();
            let absent: Vec<String> = list
                .iter()
                .filter(|n| !have.contains(n))
                .map(std::string::ToString::to_string)
                .collect();
            if !absent.is_empty() {
                return Err(Error::Missing { names: absent });
            }
            list
        } else {
            let scanned = self.scan(dir)?;
            if scanned.is_empty() {
                return Err(Error::misuse(format!(
                    "nothing to upload: {} holds no artifacts",
                    dir.display()
                )));
            }
            scanned
        };
        let mut locals = sync::local_statuses(dir, names).await;
        if gen_markers {
            up::generate_markers(dir, &mut locals).await?;
        }
        if let Some(claim) = &claim {
            let known = locals.iter().any(|(n, _)| n == claim);
            if !known {
                return Err(Error::misuse(format!(
                    "claim-first: {} is not among the scanned names of {}",
                    claim,
                    dir.display()
                )));
            }
        }
        let remotes = self
            .remote_states_for(locals.iter().map(|(n, _)| n))
            .await?;
        let d = dir.to_owned();
        let markers = gen_markers;
        let actions = tokio::task::spawn_blocking(move || {
            sync::classify(&d, Mode::Up, markers, locals, remotes)
        })
        .await
        .map_err(|e| Error::misuse(format!("task panicked: {e}")))?
        .map_err(Error::from)?;
        self.events.plan(&actions, Dir::Up);
        up::execute(
            self.client.clone(),
            dir.to_owned(),
            self.base.clone(),
            actions,
            claim,
            self.workers.clone(),
        )
        .await
    }

    /// Download into a local directory (§5.2, §5.2.1).
    ///
    /// The enumeration source is mandatory: without one the call refuses.
    /// Part files resume by default.
    /// A stale part (digest mismatch against the sibling) is discarded once and the name restarts from zero.
    /// `fresh` skips every existing part file.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when `dir` cannot be created or its orphans cleaned, [`Error::Enumerate`] when the enumeration is empty, the first [`Verdict`](crate::error::Verdict) refusal of the diff, and the first transport/auth/HTTP failure of the download.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn down(
        &self,
        dir: &Path,
        enum_src: Enumeration,
        fresh: bool,
    ) -> Result<Summary, Error> {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| Error::io(dir, e))?;
        down::cleanup_orphans(dir)?;
        let names = self.resolve_names(enum_src).await?;
        let locals = sync::local_statuses(dir, names).await;
        let remotes = self
            .remote_states_for(locals.iter().map(|(n, _)| n))
            .await?;
        let d = dir.to_owned();
        let actions = tokio::task::spawn_blocking(move || {
            sync::classify(&d, Mode::Down, true, locals, remotes)
        })
        .await
        .map_err(|e| Error::misuse(format!("task panicked: {e}")))?
        .map_err(Error::from)?;
        self.events.plan(&actions, Dir::Down);
        down::execute(
            self.client.clone(),
            dir.to_owned(),
            self.base.clone(),
            actions,
            fresh,
            self.workers.clone(),
        )
        .await
    }

    /// Delete the enumerated names from the remote directory (§5.4).
    ///
    /// The enumeration source is mandatory, like `down`.
    /// Divergence is never checked: `rm` deletes names, not content.
    /// Markers go before bytes, so nobody ever sees a complete object mid-delete.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Enumerate`] when the enumeration is empty, [`Error::ReadOnly`] on a 403/405, and the transport, auth or HTTP error that stopped the deletion walk.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn rm(&self, enum_src: Enumeration) -> Result<Summary, Error> {
        let names = self.resolve_names(enum_src).await?;
        sync::rm::execute(self.client.clone(), self.base.clone(), names).await
    }

    /// The deletion plan without deleting anything (`rm --dry-run`).
    ///
    /// Existence only: a diverging or broken remote copy still plans as `Remove`,
    /// because deletion is about names, never about content.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Enumerate`] when the enumeration is empty and transport, auth or HTTP errors while probing the remote states.
    // rm_plan fans the probes out over spawned tasks: native-only like the transfers it plans.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn rm_plan(&self, enum_src: Enumeration) -> Result<Vec<RmAction>, Error> {
        let names = self.resolve_names(enum_src).await?;
        let remotes = self.remote_states_for(&names).await?;
        Ok(names
            .into_iter()
            .map(|name| match &remotes[&name] {
                RemoteStatus::Absent => RmAction::Missing { name },
                RemoteStatus::Complete { size, .. } | RemoteStatus::Markerless { size } => {
                    RmAction::Remove { name, size: *size }
                }
                RemoteStatus::Broken(_) => RmAction::Remove { name, size: None },
            })
            .collect())
    }

    /// Resolve the enumeration source into names, refusing an empty result like `down`.
    /// A repeated explicit name collapses to its first occurrence: one artifact, one worker.
    pub async fn enumerate(&self, enum_src: Enumeration) -> Result<Vec<ArtifactName>, Error> {
        self.resolve_names(enum_src).await
    }

    /// Resolve the enumeration source into names, refusing an empty result like `down`.
    /// A repeated explicit name collapses to its first occurrence: one artifact, one worker.
    async fn resolve_names(&self, enum_src: Enumeration) -> Result<Vec<ArtifactName>, Error> {
        let names = match enum_src {
            Enumeration::Manifest(m) => m.names,
            Enumeration::Names(mut v) => {
                let mut seen = std::collections::HashSet::with_capacity(v.len());
                v.retain(|n| seen.insert(n.clone()));
                v
            }
            Enumeration::Search => layout::ls::search_assets(&self.client, &self.base).await?,
            Enumeration::Filtered { inner, prefixes } => {
                let all = Box::pin(self.resolve_names(*inner)).await?;
                let total = all.len();
                let kept: Vec<_> = all
                    .into_iter()
                    .filter(|n| prefixes.iter().any(|p| p.matches(n)))
                    .collect();
                if kept.is_empty() {
                    return Err(Error::Enumerate {
                        url: self.base.clone(),
                        reason: format!("the prefix filter kept none of {total} names"),
                    });
                }
                kept
            }
        };
        if names.is_empty() {
            return Err(Error::Enumerate {
                url: self.base.clone(),
                reason: "enumeration produced no names; the URL is probably wrong".into(),
            });
        }
        Ok(names)
    }

    /// DELETE a pointer file (`point --clear`, §5.4): absence is a normal outcome.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the DELETE; a 403/405 surfaces as [`Error::ReadOnly`].
    pub async fn point_clear(&self, url: &str) -> Result<layout::ClearOutcome, Error> {
        layout::pointer_clear(&self.client, url).await
    }

    /// List the repositories of the server this directory belongs to (`nxr service repos`).
    ///
    /// The service REST API is server metadata, not storage protocol: the call reads
    /// `<server-root>/service/rest/v1/repositories` with the same credentials as everything else.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when the base URL is not a URL, [`Error::ServiceMissing`] when the endpoint answers 404 (not a Nexus, or a version without it), and [`Error::Mismatch`] when the body is not a repository list.
    pub async fn service_repos(&self) -> Result<Vec<service::RepoInfo>, Error> {
        service::repositories(&self.client, &self.base).await
    }

    /// The mirror plan: probes both sides and classifies, moves nothing.
    /// Emits the plan event exactly like [`Nxr::mirror`](Self::mirror) does.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Enumerate`] when the enumeration is empty and the first [`Verdict`](crate::error::Verdict) refusal of the mirror diff.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn mirror_plan(
        &self,
        dst: &Nxr,
        enum_src: Enumeration,
    ) -> Result<Vec<MirrorAction>, Error> {
        let names = self.resolve_names(enum_src).await?;
        let srcs = self.remote_states_for(&names).await?;
        let dsts = dst.remote_states_for(&names).await?;
        let actions = tokio::task::spawn_blocking(move || mirror::classify(names, srcs, dsts))
            .await
            .map_err(|e| Error::misuse(format!("task panicked: {e}")))?
            .map_err(Error::from)?;
        self.events.plan_mirror(&actions);
        Ok(actions)
    }

    /// Pour enumerated names from this repository into `dst` (mirror).
    ///
    /// The enumeration lives at the source; the destination diff rules are up's:
    /// same digest skips, a different digest refuses, an unfinished copy is completed.
    /// Each copied name is staged through the down machinery (Range-aware GET, digest
    /// check) and pushed through the up machinery (PUT bytes, then PUT marker).
    /// When the enumeration lists the conventional version document
    /// ([`VERSION_DOCUMENT`](crate::sync::mirror::VERSION_DOCUMENT)) first, it is claimed:
    /// it transfers alone before any other name, and a failed claim aborts the run with
    /// nothing else sent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Enumerate`] when the enumeration is empty, the first [`Verdict`](crate::error::Verdict) refusal of the mirror diff, [`Error::Mismatch`] on a staged-body divergence, [`Error::Misuse`] if a task panicked, and [`Error::Io`] when the staging directory cannot be managed.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn mirror(&self, dst: &Nxr, enum_src: Enumeration) -> Result<Summary, Error> {
        let actions = self.mirror_plan(dst, enum_src).await?;
        // Claim-first: the version document, when the enumeration leads with it.
        let claim = mirror::claim_first(&actions);
        let staging = mirror::staging_dir(&self.base, &dst.base);
        // The parts may hold private bytes in a shared temp location: create the
        // directory private instead of publishing them to every local user.
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(&staging)
                .map_err(|e| Error::io(&staging, e))?;
        }
        #[cfg(not(unix))]
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|e| Error::io(&staging, e))?;
        // Two concurrent mirrors of one pair would interleave the same part files:
        // an advisory lock refuses the second run instead of corrupting the first.
        // The guard lives to the end of the run, the lock dies with it.
        #[cfg(unix)]
        let _staging_guard = {
            use std::os::unix::io::AsRawFd;
            let lock_path = staging.with_extension("lock");
            let lock = std::fs::File::create(&lock_path).map_err(|e| Error::io(&lock_path, e))?;
            let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc != 0 {
                return Err(Error::misuse(format!(
                    "another mirror is already running for this pair (lock: {})",
                    lock_path.display()
                )));
            }
            // `_`-prefixed on purpose: the warning is silenced, the lock is not dropped.
            Some(lock)
        };
        #[cfg(not(unix))]
        let staging_guard: Option<std::fs::File> = None;
        let result = mirror::execute(
            self.client.clone(),
            dst.client.clone(),
            staging.clone(),
            self.base.clone(),
            dst.base.clone(),
            actions,
            claim,
            self.workers.clone(),
        )
        .await;
        // A clean run consumes its staging dir; a failed one keeps the parts as the rerun's resume fuel.
        if result.is_ok() {
            if let Err(e) = tokio::fs::remove_dir_all(&staging).await {
                log::warn!("staging dir cleanup failed: {}", Error::io(&staging, e));
            }
        }
        result
    }

    /// Local verification of a directory: bytes + marker + digest, no network.
    /// Emits only the final [`Event::Summary`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] via the scan when the directory cannot be listed and [`Error::Incomplete`] naming the broken, markerless or absent entries.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn verify(
        &self,
        dir: &Path,
        names: Option<Vec<ArtifactName>>,
    ) -> Result<Summary, Error> {
        let names = match names {
            Some(v) => v,
            None => sync::scan_dir(dir)?,
        };
        let locals = sync::local_statuses(dir, names).await;
        let mut summary = Summary::default();
        for (n, st) in &locals {
            match st {
                LocalStatus::Complete(_) => summary.skipped += 1,
                LocalStatus::Markerless | LocalStatus::Broken(_) | LocalStatus::Absent => {
                    summary.failed.push(n.to_string());
                }
            }
        }
        self.events.summary(&summary);
        if !summary.failed.is_empty() {
            return Err(Error::Incomplete {
                names: summary.failed.clone(),
            });
        }
        Ok(summary)
    }

    // ---- L2: layout helpers ----------------------------------------------

    /// Read a channel ref at `url`: None on 404.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the GET and [`Error::Misuse`] when the body exceeds the small-object cap.
    pub async fn channel_get(&self, url: &str) -> Result<Option<String>, Error> {
        layout::channel_get(&self.client, url).await
    }

    /// Write a channel ref with the optional forward-only guard.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when `token` is not one line, and transport, auth or HTTP errors when reading the current value or writing the new one.
    pub async fn channel_set(
        &self,
        url: &str,
        token: &str,
        if_forward: bool,
    ) -> Result<layout::ChannelOutcome, Error> {
        layout::channel_set(&self.client, url, token, if_forward).await
    }

    /// Fetch `{base}manifest.json`: the conventional enumeration source.
    /// None when the server has no manifest there.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the GET and the [`Manifest::from_slice`] parse errors.
    pub async fn manifest_at_base(&self) -> Result<Option<Manifest>, Error> {
        let url = format!("{}manifest.json", self.base);
        match self.client.get_small(&url).await? {
            None => Ok(None),
            Some(bytes) => Manifest::from_slice(&bytes).map(Some),
        }
    }

    /// Fetch a manifest from an arbitrary URL (`--manifest <url>`).
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the GET (a missing object is [`Error::Http`] 404) and the [`Manifest::from_slice`] parse errors.
    pub async fn manifest_from(&self, url: &str) -> Result<Manifest, Error> {
        Manifest::from_url(&self.client, url).await
    }

    /// The immediate children of a raw directory URL (`nxr ls`): folders first, then files.
    ///
    /// A raw repository is an arbitrary tree: this is the navigation primitive,
    /// usable at any depth. Leaf `.sha256` siblings are hidden as derived data.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when the base URL is not a directory URL and [`Error::Enumerate`] when the search endpoint is unavailable.
    pub async fn ls_entries(&self) -> Result<Vec<layout::ls::Entry>, Error> {
        layout::ls::search_entries(&self.client, &self.base).await
    }

    /// Version tokens under this base via the search API (experimental, the version view).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when the base is not a directory URL and [`Error::Enumerate`] when the search endpoint is unavailable or unparseable.
    pub async fn ls_versions(&self) -> Result<Vec<String>, Error> {
        layout::ls::search_versions(&self.client, &self.base).await
    }

    /// Artifact names under this base via the search API (experimental).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when the base is not a directory URL and [`Error::Enumerate`] when the search endpoint is unavailable or unparseable.
    pub async fn ls_assets(&self) -> Result<Vec<ArtifactName>, Error> {
        layout::ls::search_assets(&self.client, &self.base).await
    }

    // ---- internals ---------------------------------------------------------

    /// Remote states for the given names, in order.
    #[cfg(not(target_arch = "wasm32"))]
    async fn remote_states_for<'a>(
        &self,
        names: impl IntoIterator<Item = &'a ArtifactName>,
    ) -> Result<BTreeMap<ArtifactName, RemoteStatus>, Error> {
        let names: Vec<ArtifactName> = names.into_iter().cloned().collect();
        let mut set = tokio::task::JoinSet::new();
        for (i, n) in names.iter().enumerate() {
            let permit = self
                .workers
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| Error::misuse(format!("worker semaphore closed: {e}")))?;
            let client = self.client.clone();
            let dir = self.base.clone();
            let n = n.clone();
            set.spawn(async move {
                let _permit = permit;
                (i, client.probe(&dir, &n).await.map(|s| (n, s)))
            });
        }
        let mut slots: Vec<Option<(ArtifactName, RemoteStatus)>> =
            (0..names.len()).map(|_| None).collect();
        while let Some(res) = set.join_next().await {
            let (i, inner) = res.map_err(|e| Error::misuse(format!("task panicked: {e}")))?;
            slots[i] = Some(inner?);
        }
        Ok(slots
            .into_iter()
            .map(|s| s.expect("all slots filled"))
            .collect())
    }
}
