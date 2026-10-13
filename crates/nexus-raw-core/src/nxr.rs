//! The `Nxr` facade: the single entry point for CLI and wrappers.
//!
//! On wasm32 the surface carries the read paths only: `ls`, `head`,
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
use crate::layout;
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
    /// Explicit names (`--name`, repeatable).
    Names(Vec<ArtifactName>),
    /// Best-effort search-API traversal (`--ls`).
    Search,
    /// An enumeration narrowed to whole-segment prefixes (the `--prefix` filter).
    /// The filter applies after the inner source resolves and dedups.
    Filtered {
        /// The enumeration the filter applies to.
        inner: Box<Enumeration>,
        /// Prefixes: a name survives when it matches at least one.
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
    /// The AIMD budget of the most recent directory transfer, kept for
    /// introspection: conformance tests assert the limit was reduced after 429s.
    last_aimd: Arc<std::sync::Mutex<Option<Arc<crate::transport::aimd::WorkerLimit>>>>,
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
            last_aimd: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// The normalized base URL of this invocation.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The configured worker maximum: the AIMD budget starts here and never
    /// exceeds it.
    #[must_use]
    fn max_workers(&self) -> usize {
        self.client.workers_max()
    }

    /// The worker limit of the most recent directory transfer: the current AIMD
    /// value and the lowest value the run reached.
    /// Test and diagnosis surface for the §7 adaptive worker budget.
    #[must_use]
    pub fn last_run_limit(&self) -> (usize, usize) {
        let guard = self.last_aimd.lock().expect("last aimd lock");
        match guard.as_ref() {
            Some(aimd) => (aimd.limit(), aimd.min_limit()),
            None => (self.max_workers(), self.max_workers()),
        }
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

    /// GET a URL in `segments` concurrent byte-range pieces (spec §5.1 `get --segments`).
    ///
    /// Falls back to the single-stream [`Nxr::get`] result shape when the server ignores `Range`
    /// (the caller reruns the plain path; the error here is [`Error::RangeIgnored`]).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Nxr::get`] plus any segment failure (transport, auth, HTTP),
    /// [`Error::Mismatch`] on a sibling digest divergence, and [`Error::Misuse`] for `segments < 2`.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn get_segmented(
        &self,
        url: &str,
        out: PathBuf,
        segments: usize,
    ) -> Result<GetOutcome, Error> {
        if segments < 2 {
            return Err(Error::misuse(format!(
                "segments must be at least 2, got {segments}: a single span is the plain get"
            )));
        }
        match primitive::get_segmented(&self.client, url, &out, segments, &self.events).await {
            // The server answers ranged GETs with the whole body: the plain path is the answer,
            // and the fallback is progress, never an error. The switch travels the events channel.
            Err(Error::RangeIgnored { .. }) => {
                self.events
                    .sender()
                    .send(Event::Retrying {
                        name: out
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        attempt: 1,
                        reason: "the server ignores Range: falling back to a single stream"
                            .to_owned(),
                    })
                    .ok();
                primitive::get(&self.client, url, Some(out), false).await
            }
            other => other,
        }
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

    /// The delta report built from the search API's per-asset facts (`diff --via search`).
    ///
    /// The remote side comes from the search index: name, digest and size per
    /// asset, O(pages) requests instead of a HEAD/sibling pair per name.
    /// The report shape is the plain [`Self::delta`] one.
    /// Honesty: a searched name without a search-reported digest reports as
    /// [`Delta::Diverged`] with `sha: false`, never as `Same`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] and [`Error::UnsafeName`] from the local scan,
    /// [`Error::SearchRepoMissing`] on a search 400 and [`Error::Enumerate`] on a
    /// search 404 or an unparseable page, and [`Error::Misuse`] if a task panicked.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn delta_search(
        &self,
        dir: &Path,
        prefixes: &[crate::model::name::NamePrefix],
    ) -> Result<Vec<Delta>, Error> {
        let local_names = sync::scan_dir(dir)?;
        let locals = sync::local_statuses(dir, local_names).await;
        let mut remotes = layout::ls::search_assets_facts(&self.client, &self.base).await?;
        if !prefixes.is_empty() {
            remotes.retain(|(n, _)| prefixes.iter().any(|p| p.matches(n)));
        }
        let d = dir.to_owned();
        tokio::task::spawn_blocking(move || delta::compare_search(&d, locals, remotes))
            .await
            .map_err(|e| Error::misuse(format!("task panicked: {e}")))
    }

    /// Audit the repository at the facade base (§11).
    ///
    /// The structural pass lists the raw tree through the search API (markers
    /// included), fetches every listed marker body (small GETs, worker-paced)
    /// and classifies the integrity classes over the metadata; asset bytes are
    /// never read. The search facts double as the independent cross-check of
    /// every parsed marker (`diverged_fact`), so a run over a live index both
    /// proves existence and cross-checks content.
    ///
    /// `bytes_mode` additionally downloads and hashes every asset against its
    /// marker: the only pass that sees silent bitrot. `sample` limits the byte
    /// pass to a deterministic subset of that size; `None` or `Some(0)` checks
    /// everything.
    ///
    /// The repository is only read: no audit request mutates it.
    ///
    /// # Errors
    ///
    /// Returns the search enumeration errors ([`Error::SearchRepoMissing`],
    /// [`Error::Enumerate`]), the first transport/auth failure from the marker
    /// or byte fetches, and [`Error::Misuse`] when a task panicked.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn audit(
        &self,
        bytes_mode: bool,
        sample: Option<usize>,
    ) -> Result<sync::audit::AuditRun, Error> {
        use sync::audit::{AuditInput, ByteCheck};

        // 1. Discovery: the whole-tree search walk, markers included.
        let tree_list = layout::ls::audit_tree(&self.client, &self.base).await?;
        let facts: BTreeMap<ArtifactName, Option<Digest>> = tree_list
            .iter()
            .filter_map(|(p, d)| {
                // Grammar-clean, non-marker names carry a cross-check fact.
                (d.is_some() && !p.ends_with(".sha256"))
                    .then(|| ArtifactName::parse(p).ok().map(|n| (n, d.clone())))
                    .flatten()
            })
            .collect();

        // 2. Marker bodies for every listed marker, worker-paced.
        let marker_keys: Vec<String> = tree_list
            .iter()
            .filter(|(p, _)| p.ends_with(".sha256"))
            .map(|(p, _)| p.clone())
            .collect();
        let mut set = tokio::task::JoinSet::new();
        for (i, key) in marker_keys.iter().enumerate() {
            let permit = self
                .workers
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| Error::misuse(format!("worker semaphore closed: {e}")))?;
            let client = self.client.clone();
            let base = self.base.clone();
            let key = key.clone();
            set.spawn(async move {
                let _permit = permit;
                let marker_url = format!("{base}{}", encode_rel(&key));
                let body = client.get_small(&marker_url).await;
                (
                    i,
                    body.map(|b| b.map(|bytes| String::from_utf8_lossy(&bytes).into_owned())),
                )
            });
        }
        let mut bodies: Vec<Option<Option<String>>> =
            (0..marker_keys.len()).map(|_| None).collect();
        while let Some(res) = set.join_next().await {
            let (i, inner) = res.map_err(|e| Error::misuse(format!("task panicked: {e}")))?;
            bodies[i] = Some(inner?);
        }
        let _ = &mut bodies;

        // 3. Fold the bodies into the tree map the classifier reads.
        let mut tree: BTreeMap<String, Option<String>> =
            tree_list.iter().map(|(p, _)| (p.clone(), None)).collect();
        for (i, key) in marker_keys.iter().enumerate() {
            if let Some(slot) = tree.get_mut(key) {
                *slot = bodies[i].clone().flatten();
            }
        }

        // 4. The structural classification (pure).
        let input = AuditInput { tree, facts };
        let verdict = sync::audit::classify(&input);

        // 5. The byte pass: download and hash against the parsed marker.
        let mut byte_checks: BTreeMap<ArtifactName, ByteCheck> = BTreeMap::new();
        if bytes_mode {
            let asset_keys: Vec<String> = tree_list
                .iter()
                .filter(|(p, _)| !p.ends_with(".sha256"))
                .map(|(p, _)| p.clone())
                .collect();
            let chosen: Vec<String> = match sample {
                Some(n) if n > 0 && n < asset_keys.len() => {
                    // Deterministic stride sample: sort is stable above.
                    let step = asset_keys.len() / n;
                    asset_keys
                        .iter()
                        .step_by(step.max(1))
                        .take(n)
                        .cloned()
                        .collect()
                }
                _ => asset_keys,
            };
            let mut bset = tokio::task::JoinSet::new();
            for (i, key) in chosen.iter().enumerate() {
                let permit = self
                    .workers
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| Error::misuse(format!("worker semaphore closed: {e}")))?;
                let client = self.client.clone();
                let base = self.base.clone();
                let key = key.clone();
                bset.spawn(async move {
                    let _permit = permit;
                    let url = format!("{base}{}", encode_rel(&key));
                    let actual =
                        crate::primitive::sha(&client, crate::primitive::ShaSource::Url(url)).await;
                    (i, actual)
                });
            }
            let mut slots: Vec<Option<Result<Digest, Error>>> =
                (0..chosen.len()).map(|_| None).collect();
            while let Some(res) = bset.join_next().await {
                let (i, inner) = res.map_err(|e| Error::misuse(format!("task panicked: {e}")))?;
                slots[i] = Some(inner);
            }
            for (i, key) in chosen.iter().enumerate() {
                let actual = slots[i].take().expect("slot filled")?;
                let marker_key = format!("{}.sha256", key);
                let parsed = input
                    .tree
                    .get(&marker_key)
                    .cloned()
                    .flatten()
                    .as_deref()
                    .and_then(sync::audit::parse_marker_body);
                let Ok(asset_name) = ArtifactName::parse(key) else {
                    continue;
                };
                let check = match parsed {
                    Some((marker, _)) if marker == actual => ByteCheck::Match,
                    Some((marker, _)) => ByteCheck::Mismatch { marker, actual },
                    None => continue,
                };
                byte_checks.insert(asset_name, check);
            }
        }

        Ok(sync::audit::AuditRun {
            verdict,
            byte_checks,
        })
    }

    /// Upload a local directory (§5.2).
    /// `names` restricts the transfer to the explicit list.
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
        // One AIMD budget per run: workers shrink on `Retry-After` (§7) and
        // recover additively, capped at the configured maximum.
        let aimd = Arc::new(crate::transport::aimd::WorkerLimit::new(self.max_workers()));
        self.client.attach_aimd(aimd.clone());
        *self.last_aimd.lock().expect("last aimd lock") = Some(aimd.clone());
        up::execute(
            self.client.clone(),
            dir.to_owned(),
            self.base.clone(),
            actions,
            claim,
            aimd,
        )
        .await
    }

    /// Upload a local directory, skipping names the remote already vouches for (`up --if-changed`).
    ///
    /// The plan is [`sync::classify_if_changed`](crate::sync): the only skip is a complete
    /// local copy whose digest equals the remote marker's digest, everything divergent
    /// re-PUTs, and a broken local or remote object refuses the run exactly like the plain `up`:
    /// the flag relaxes the divergence rule, never the never-overwrite rule.
    ///
    /// # Errors
    ///
    /// The same classes as [`Self::up`].
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn up_if_changed(
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
            sync::classify_if_changed(&d, markers, locals, remotes)
        })
        .await
        .map_err(|e| Error::misuse(format!("task panicked: {e}")))?
        .map_err(Error::from)?;
        self.events.plan(&actions, Dir::Up);
        // One AIMD budget per run, the same contract as the plain `up`.
        let aimd = Arc::new(crate::transport::aimd::WorkerLimit::new(self.max_workers()));
        self.client.attach_aimd(aimd.clone());
        *self.last_aimd.lock().expect("last aimd lock") = Some(aimd.clone());
        up::execute(
            self.client.clone(),
            dir.to_owned(),
            self.base.clone(),
            actions,
            claim,
            aimd,
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
        let aimd = Arc::new(crate::transport::aimd::WorkerLimit::new(self.max_workers()));
        self.client.attach_aimd(aimd.clone());
        *self.last_aimd.lock().expect("last aimd lock") = Some(aimd.clone());
        down::execute(
            self.client.clone(),
            dir.to_owned(),
            self.base.clone(),
            actions,
            fresh,
            aimd,
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
    /// Returns transport, auth or HTTP errors from the DELETE.
    /// A 403/405 surfaces as [`Error::ReadOnly`].
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

    /// Server liveness and writability, resolved from this client's base.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors.
    /// A refused probe reads as `false`, not as an error.
    pub async fn service_status(&self) -> Result<service::StatusReport, Error> {
        service::status(&self.client, &self.base).await
    }

    /// The repository this base belongs to, with optional admin-only settings.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ServiceMissing`] when the collection endpoint answers 404, [`Error::Missing`] when no collection entry matches the base, and [`Error::Mismatch`] when a body is not the expected JSON.
    pub async fn service_repo(&self, detail: bool) -> Result<service::RepoReport, Error> {
        service::repo(&self.client, &self.base, detail).await
    }

    /// Every asset of the repository this base belongs to, through the search API.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when the base is not a repository URL, [`Error::Enumerate`] when the search endpoint is absent or lies, and transport/auth errors as they come.
    pub async fn service_assets(
        &self,
        q: Option<&str>,
        prefix: &[String],
    ) -> Result<(Vec<service::AssetEntry>, service::AssetsSummary), Error> {
        service::assets(&self.client, &self.base, q, prefix).await
    }

    /// The EULA gate state: `None` when the server has no gate.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors, and [`Error::Mismatch`] when the body is not the EULA document.
    pub async fn service_eula(&self) -> Result<Option<service::EulaStatus>, Error> {
        service::eula(&self.client, &self.base).await
    }

    /// Ensure the EULA gate is open: accept the disclaimer when presented.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors.
    /// The accept POST carries the echoed disclaimer.
    pub async fn service_eula_accept(&self) -> Result<service::EulaOutcome, Error> {
        service::eula_accept(&self.client, &self.base).await
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
    /// The enumeration lives at the source.
    /// The destination diff rules are up's:
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
        // No mode knob off unix: the dir inherits the privacy of the user's
        // own temp location, which is where it lives anyway.
        #[cfg(not(unix))]
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|e| Error::io(&staging, e))?;
        // Two concurrent mirrors of one pair would interleave the same part files:
        // an advisory lock refuses the second run instead of corrupting the first.
        // The guard lives to the end of the run, the lock dies with it.
        let _staging_guard = acquire_staging_lock(&staging)?;
        let result = {
            // One AIMD budget per run, covering both legs: the source GET and
            // the destination PUT report into the same controller (§7).
            let aimd = Arc::new(crate::transport::aimd::WorkerLimit::new(self.max_workers()));
            self.client.attach_aimd(aimd.clone());
            dst.client.attach_aimd(aimd.clone());
            *self.last_aimd.lock().expect("last aimd lock") = Some(aimd.clone());
            mirror::execute(
                self.client.clone(),
                dst.client.clone(),
                staging.clone(),
                self.base.clone(),
                dst.base.clone(),
                actions,
                claim,
                aimd,
            )
            .await
        };
        // A clean run consumes its staging dir.
        // A failed one keeps the parts as the rerun's resume fuel.
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

    /// [`Self::ls_entries`] plus the number of `.sha256` markers the enumerator hid.
    ///
    /// # Errors
    ///
    /// The same as [`Self::ls_entries`].
    pub async fn ls_entries_counted(&self) -> Result<(Vec<layout::ls::Entry>, usize), Error> {
        layout::ls::search_entries_counted(&self.client, &self.base).await
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

/// Take the exclusive staging lock of a `mirror` run.
///
/// The lock is the file handle itself, so it dies with the run and a crashed
/// run leaves nothing to clean up: the file is never unlinked while held.
/// A live twin's open fails, and the refusal is misuse naming the lock path.
///
/// Platform arms:
/// - unix: an exclusive `flock`. The first create is exclusive (`create_new`),
///   which also refuses a symlink plant at the predictable temp name; a dead
///   run's leftover file is taken over once the flock comes free.
/// - windows: the file is opened with read and write sharing denied, so a
///   second open for writing fails while this run lives. Share-delete stays
///   granted, otherwise the clean run's staging cleanup could not unlink the
///   file this very handle still holds.
///
/// # Errors
///
/// Returns [`Error::Misuse`] when another run holds the lock and [`Error::Io`] when the lock file cannot be managed.
#[cfg(not(target_arch = "wasm32"))]
fn acquire_staging_lock(staging: &Path) -> Result<std::fs::File, Error> {
    let path = staging.join(".lock");
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // The lock sits inside the 0700 staging directory, not next to it:
        // a predictable name in a world-writable temp dir is a symlink
        // plant, and File::create follows symlinks. create_new
        // (O_CREAT|O_EXCL) refuses an existing entry outright.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(Error::io(&path, e)),
        }
        // A lock from a dead run (a crash never unlocked): take over only
        // when the flock comes free. A live run keeps holding it.
        let existing = std::fs::File::open(&path).map_err(|e| Error::io(&path, e))?;
        let rc = unsafe { libc::flock(existing.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(existing);
        }
    }
    #[cfg(not(unix))]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .share_mode(0x4) // FILE_SHARE_DELETE: see the arm doc above.
            .open(&path);
        match opened {
            Ok(file) => return Ok(file),
            // 32: ERROR_SHARING_VIOLATION, a live holder of the lock file.
            Err(e) if e.raw_os_error() == Some(32) => {}
            Err(e) => return Err(Error::io(&path, e)),
        }
    }
    Err(Error::misuse(format!(
        "another mirror is already running for this pair (lock: {})",
        path.display()
    )))
}

/// Percent-encode a repository-relative path for a URL: safe characters pass,
/// everything else goes percent-hex per segment (separators kept).
/// Marker names cannot go through [`ArtifactName::encoded`](crate::model::name::ArtifactName::encoded),
/// because the grammar refuses the reserved suffix.
fn encode_rel(rel: &str) -> String {
    rel.split('/')
        .map(|seg| {
            let mut out = String::with_capacity(seg.len());
            for b in seg.bytes() {
                match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' | b'~' => {
                        out.push(b as char)
                    }
                    other => out.push_str(&format!("%{other:02X}")),
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("/")
}
