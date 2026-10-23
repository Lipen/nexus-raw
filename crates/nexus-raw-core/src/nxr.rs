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
use crate::error::DATA_EXIT;
#[cfg(not(target_arch = "wasm32"))]
use crate::events::{Dir, Failure, Summary};
use crate::events::{Event, Progress};
use crate::layout;
use crate::model::digest::Digest;
use crate::model::name::ArtifactName;
#[cfg(not(target_arch = "wasm32"))]
use crate::model::state::LocalStatus;
#[cfg(not(target_arch = "wasm32"))]
use crate::model::state::RemoteStatus;
use crate::primitive;
use crate::primitive::HeadInfo;
#[cfg(not(target_arch = "wasm32"))]
use crate::primitive::{GetOutcome, RangeOutcome, ShaSource};
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

fn validate_explicit_names(dir: &Path, names: &[ArtifactName]) -> Result<(), Error> {
    let mut absent = Vec::new();
    for name in names {
        let mut cur = dir.to_path_buf();
        let segments: Vec<&str> = name.as_str().split('/').collect();
        let mut missing = false;
        for (i, seg) in segments.iter().enumerate() {
            if seg.starts_with('.') {
                missing = true;
                break;
            }
            cur.push(seg);
            let meta = match std::fs::symlink_metadata(&cur) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    missing = true;
                    break;
                }
                Err(e) => return Err(Error::io(&cur, e)),
            };
            if meta.file_type().is_symlink() {
                return Err(Error::UnsafeName {
                    name: name.to_string(),
                    reason: "symlinked entry: upload the target, not the link".to_owned(),
                });
            }
            if i + 1 == segments.len() {
                if meta.is_dir() {
                    missing = true;
                } else if !meta.is_file() {
                    return Err(Error::UnsafeName {
                        name: name.to_string(),
                        reason: "non-regular entry: only files and directories are artifacts"
                            .to_owned(),
                    });
                }
            } else if !meta.is_dir() {
                missing = true;
            }
            if missing {
                break;
            }
        }
        if missing {
            absent.push(name.to_string());
        }
    }
    if absent.is_empty() {
        Ok(())
    } else {
        Err(Error::Missing { names: absent })
    }
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
    /// (the caller reruns the plain path. The error here is [`Error::RangeIgnored`]).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Nxr::get`] plus any segment failure (transport, auth, HTTP),
    /// [`Error::Mismatch`] on a sibling digest divergence, and [`Error::Misuse`] outside `2..=64`.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn get_segmented(
        &self,
        url: &str,
        out: PathBuf,
        segments: usize,
    ) -> Result<GetOutcome, Error> {
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

    /// PUT an in-memory body, optionally with its sha-sibling marker
    /// (spec §5.1: the `put --stdin` path, and the browser upload path on wasm).
    ///
    /// The bytes are already whole in memory: Content-Length is just `body.len()`,
    /// and the digest is computed on the buffer, no extra pass.
    /// With `sha`, the marker is PUT right after the bytes, naming the object
    /// by its final path segment: the same order and shape as [`Nxr::put`].
    /// Returns `(bytes sent, digest when sha)`.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the uploads and
    /// [`Error::UnsafeName`] when the final URL segment cannot carry a marker.
    pub async fn put_bytes(
        &self,
        url: &str,
        body: &[u8],
        sha: bool,
    ) -> Result<(u64, Option<Digest>), Error> {
        let digest = Digest::of_bytes(body);
        // Validate the marker name before the first PUT: a bytes-first order
        // would leave an unverifiable object behind when the name is unsafe.
        let marker = if sha {
            let marker_name = url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default();
            let name = ArtifactName::parse(marker_name).map_err(|e| Error::UnsafeName {
                name: marker_name.to_owned(),
                reason: format!("the final URL segment cannot carry a sha marker: {e}"),
            })?;
            Some(crate::model::sibling::format_line(name.as_str(), &digest))
        } else {
            None
        };
        self.client.put_small(url, body.to_vec()).await?;
        let Some(marker) = marker else {
            return Ok((body.len() as u64, None));
        };
        let sib_url = format!("{url}.sha256");
        self.client.put_small(&sib_url, marker.into_bytes()).await?;
        Ok((body.len() as u64, Some(digest)))
    }

    /// HEAD a URL (spec §5.1).
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors after the retry loop is exhausted.
    pub async fn head(&self, url: &str) -> Result<HeadInfo, Error> {
        primitive::head(&self.client, url).await
    }

    /// Read a byte range into memory (the `nxr-mount` read path).
    ///
    /// `None` means the object is absent.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors, and [`Error::Misuse`] when `start + len` overflows.
    pub async fn read_range(
        &self,
        url: &str,
        start: u64,
        len: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        if len == 0 {
            // The contract says `None` means absent: a zero-length read still
            // has to know, so probe the object with a HEAD.
            let info = self.head(url).await?;
            return Ok((info.status != 404).then(Vec::new));
        }
        let Some(end_incl) = start.checked_add(len - 1) else {
            return Err(Error::misuse("the byte range overflows u64"));
        };
        self.client.get_range(url, start, end_incl).await
    }

    /// Read one `START-END` or `START-` byte range.
    ///
    /// The open form learns the size with a HEAD; a start at or past the end
    /// is a misuse, not a successful empty read.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Missing`] when the object is absent,
    /// [`Error::Misuse`] for a malformed or past-the-end range, and the
    /// transport/auth/HTTP errors of the HEAD or GET.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn get_range(&self, url: &str, spec: &str) -> Result<RangeOutcome, Error> {
        let (start, end) = primitive::parse_range(spec)?;
        let len = match end {
            Some(end) => end
                .checked_sub(start)
                .and_then(|d| d.checked_add(1))
                .ok_or_else(|| Error::Misuse("the byte range overflows u64".to_owned()))?,
            None => {
                let info = self.head(url).await?;
                if info.status == 404 {
                    return Err(Error::Missing {
                        names: vec![url.to_owned()],
                    });
                }
                let size = info.size.ok_or_else(|| {
                    Error::Misuse(
                        "the server did not report a size: use START-END instead".to_owned(),
                    )
                })?;
                if start >= size {
                    return Err(Error::Misuse(format!(
                        "range starts at {start}, past the end of {size} bytes"
                    )));
                }
                size - start
            }
        };
        let bytes = self
            .read_range(url, start, len)
            .await?
            .ok_or_else(|| Error::Missing {
                names: vec![url.to_owned()],
            })?;
        // A 416 reaches the core as an empty span: a start at or past the end
        // is not a successful empty read.
        if bytes.is_empty() && len > 0 {
            let info = self.head(url).await?;
            if info.status == 404 {
                return Err(Error::Missing {
                    names: vec![url.to_owned()],
                });
            }
            if let Some(size) = info.size {
                if start >= size {
                    return Err(Error::Misuse(format!(
                        "range starts at {start}, past the end of {size} bytes"
                    )));
                }
            }
        }
        Ok(RangeOutcome { start, end, bytes })
    }

    /// The `.sha256` marker digest of one object, when the server has a marker.
    ///
    /// A missing marker is `None`; a marker outside the `sha256sum -c` format is
    /// [`Error::Mismatch`], because a digest that cannot be read cannot verify
    /// anything.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the marker read, and
    /// [`Error::Mismatch`] when the marker does not parse.
    pub async fn sibling_digest(&self, url: &str) -> Result<Option<Digest>, Error> {
        let marker_url = format!("{url}.sha256");
        let Some(raw) = self.client.get_small(&marker_url).await? else {
            return Ok(None);
        };
        let text = String::from_utf8_lossy(&raw);
        let parsed =
            crate::model::sibling::parse_line(&text).map_err(|reason| Error::Mismatch {
                name: url.to_owned(),
                detail: format!("the .sha256 marker does not parse: {reason}"),
            })?;
        Ok(Some(parsed.digest))
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

    /// The artifact names under the selected whole-segment prefixes.
    ///
    /// Only the selected subtrees are read: an unselected symlink or unsafe
    /// name cannot refuse the run. A prefix must name a real directory whose
    /// components are not symlinks; a hidden prefix is a misuse.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] for a bad or missing prefix, [`Error::UnsafeName`]
    /// for a symlink, and [`Error::Io`] from the walk.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn scan_prefixes(
        &self,
        dir: &Path,
        prefixes: &[String],
    ) -> Result<Vec<ArtifactName>, Error> {
        let parsed = prefixes
            .iter()
            .map(|p| crate::model::name::NamePrefix::parse(p))
            .collect::<Result<Vec<_>, _>>()?;
        sync::scan_dir_prefixes(dir, &parsed)
    }

    /// Resolve the names an `up` works on: an explicit list is validated path
    /// by path, `None` scans the whole directory. The explicit path never
    /// walks unrelated entries, so a caller that already scoped the tree pays
    /// for its names only.
    fn resolve_up_names(
        &self,
        dir: &Path,
        names: Option<Vec<ArtifactName>>,
    ) -> Result<Vec<ArtifactName>, Error> {
        match names {
            Some(list) => {
                validate_explicit_names(dir, &list)?;
                Ok(list)
            }
            None => {
                let scanned = self.scan(dir)?;
                if scanned.is_empty() {
                    return Err(Error::misuse(format!(
                        "nothing to upload: {} holds no artifacts",
                        dir.display()
                    )));
                }
                Ok(scanned)
            }
        }
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
    /// and classifies the integrity classes over the metadata. Asset bytes are
    /// never read. The search facts double as the independent cross-check of
    /// every parsed marker (`diverged_fact`), so a run over a live index both
    /// proves existence and cross-checks content.
    ///
    /// `bytes_mode` additionally downloads and hashes every asset against its
    /// marker: the only pass that sees silent bitrot. `sample` limits the byte
    /// pass to a deterministic subset of that size. `None` or `Some(0)` checks
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
        self.audit_filtered(bytes_mode, sample, &sync::audit::AuditFilter::default())
            .await
    }

    /// Audit a narrowed tree: [`Nxr::audit`] with an
    /// [`AuditFilter`](sync::audit::AuditFilter) applied at discovery.
    ///
    /// The filter runs before any request: markers outside the selected names
    /// or prefixes are never fetched, the byte pass samples only the selected
    /// assets, and `audited` counts the selected objects.
    ///
    /// # Errors
    ///
    /// The errors of [`Nxr::audit`].
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn audit_filtered(
        &self,
        bytes_mode: bool,
        sample: Option<usize>,
        filter: &sync::audit::AuditFilter,
    ) -> Result<sync::audit::AuditRun, Error> {
        use sync::audit::{AuditInput, ByteCheck};

        // 1. Discovery: the whole-tree search walk, markers included, then
        // the client-side narrowing before a single marker is fetched.
        let listed = layout::ls::audit_tree(&self.client, &self.base).await?;
        // The cross-check facts come from the whole listing: a filter that
        // selects nothing must not report a live index as unavailable.
        let facts: BTreeMap<ArtifactName, Option<Digest>> = listed
            .iter()
            .filter_map(|(p, d)| {
                // Grammar-clean, non-marker names carry a cross-check fact.
                (d.is_some() && !p.ends_with(".sha256"))
                    .then(|| ArtifactName::parse(p).ok().map(|n| (n, d.clone())))
                    .flatten()
            })
            .collect();
        let tree_list: Vec<_> = listed
            .into_iter()
            .filter(|(p, _)| filter.matches_key(p))
            .collect();
        let audited = tree_list
            .iter()
            .filter(|(p, _)| !p.ends_with(".sha256"))
            .count();

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
            audited,
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
    /// Returns [`Error::Io`] and [`Error::UnsafeName`] from the scan or the explicit-name check, [`Error::Misuse`] when `dir` is not a directory, the scan is empty, or the claim is unknown, and [`Error::Missing`] when a requested name is absent from the tree.
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
        let names = self.resolve_up_names(dir, names)?;
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
    pub async fn up_if_changed_plan(
        &self,
        dir: &Path,
        names: Option<Vec<ArtifactName>>,
        gen_markers: bool,
        claim: Option<ArtifactName>,
    ) -> Result<Vec<Action>, Error> {
        if !dir.is_dir() {
            return Err(Error::misuse(format!("not a directory: {}", dir.display())));
        }
        let names = self.resolve_up_names(dir, names)?;
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
        Ok(actions)
    }

    /// Upload a local directory, skipping names the remote already vouches for (`up --if-changed`).
    ///
    /// The plan is [`Self::up_if_changed_plan`]. This method emits it and executes it.
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
        let actions = self
            .up_if_changed_plan(dir, names, gen_markers, claim.clone())
            .await?;
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
        self.down_with_segments(dir, enum_src, fresh, None).await
    }

    /// Download with each artifact split into `segments` concurrent Range
    /// spans (`down --segments`).
    ///
    /// The worker budget still caps the network concurrency: the flag widens
    /// one object, never the run. A zero-byte object, a split that cannot give
    /// every span a byte, or a server that ignores Range falls back to the
    /// plain single-stream path.
    /// A finished span survives an interrupted run, so a rerun fetches only
    /// the spans that did not finish.
    ///
    /// # Errors
    ///
    /// The same as [`Self::down`], plus [`Error::Misuse`] when `segments` is
    /// outside 2..=64.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn down_with_segments(
        &self,
        dir: &Path,
        enum_src: Enumeration,
        fresh: bool,
        segments: Option<usize>,
    ) -> Result<Summary, Error> {
        if let Some(n) = segments {
            if !(2..=crate::primitive::MAX_SEGMENTS).contains(&n) {
                return Err(Error::misuse(format!(
                    "segments must be between 2 and {}, got {n}",
                    crate::primitive::MAX_SEGMENTS
                )));
            }
        }
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
            segments,
        )
        .await
    }

    /// The clone plan: every repository artifact against the local tree.
    ///
    /// The enumeration is the search API walked at the invocation base, so a
    /// repository root works as well as a version directory.
    /// A local fact cache (`StateMode`) skips re-hashing files whose size and
    /// mtime still match the last verified copy. `verify` ignores it.
    /// Nothing is written.
    ///
    /// # Errors
    ///
    /// Returns the search enumeration errors, the first transport/auth failure
    /// of the remote probes, and the first [`Error::Mismatch`] or
    /// [`Error::Missing`] verdict.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn clone_plan(
        &self,
        dir: &Path,
        prefixes: &[crate::model::name::NamePrefix],
        state: crate::sync::StateMode<'_>,
        verify: bool,
    ) -> Result<Vec<crate::sync::CloneAction>, Error> {
        Ok(self.clone_prepare(dir, prefixes, state, verify).await?.0)
    }

    /// Clone the repository at the invocation base into `dir`.
    ///
    /// Downloads ride the verified `down` machinery, a local object whose
    /// bytes already match its remote marker gets its missing marker written
    /// (`sealed`), and a complete matching copy is skipped.
    /// A local file that diverges from its own marker is never overwritten:
    /// the run refuses with exit 1.
    ///
    /// # Errors
    ///
    /// The same as [`Self::clone_plan`], plus the first transport/auth/HTTP
    /// failure of the downloads.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn clone_to(
        &self,
        dir: &Path,
        prefixes: &[crate::model::name::NamePrefix],
        state_mode: crate::sync::StateMode<'_>,
        verify: bool,
    ) -> Result<Summary, Error> {
        let (actions, state, state_path) = self
            .clone_prepare(dir, prefixes, state_mode, verify)
            .await?;
        self.events.plan_clone(&actions);
        let aimd = Arc::new(crate::transport::aimd::WorkerLimit::new(self.max_workers()));
        self.client.attach_aimd(aimd.clone());
        *self.last_aimd.lock().expect("last aimd lock") = Some(aimd.clone());
        sync::clone::execute(
            self.client.clone(),
            dir.to_owned(),
            self.base.clone(),
            actions,
            state,
            state_path,
            aimd,
        )
        .await
    }

    /// The shared clone preparation: enumerate, classify, load the cache.
    #[cfg(not(target_arch = "wasm32"))]
    async fn clone_prepare(
        &self,
        dir: &Path,
        prefixes: &[crate::model::name::NamePrefix],
        state_mode: crate::sync::StateMode<'_>,
        verify: bool,
    ) -> Result<
        (
            Vec<crate::sync::CloneAction>,
            crate::sync::clone::CloneState,
            Option<PathBuf>,
        ),
        Error,
    > {
        let names: Vec<ArtifactName> =
            crate::layout::ls::search_repo_assets(&self.client, &self.base)
                .await?
                .into_iter()
                .map(|(name, _, _)| name)
                .filter(|name| prefixes.is_empty() || prefixes.iter().any(|p| p.matches(name)))
                .collect();
        if names.is_empty() {
            return Err(Error::Enumerate {
                url: self.base.clone(),
                reason: "the repository holds no artifacts under the selected prefixes".into(),
            });
        }
        let state_path = state_mode.path(dir);
        let state = state_path.as_deref().map_or_else(
            || sync::clone::CloneState::empty(&self.base),
            |path| sync::clone::CloneState::load(path, &self.base),
        );
        let locals = sync::clone::local_facts(dir, names.clone(), state.clone(), verify).await;
        let remotes = self.remote_states_for(names.iter()).await?;
        let actions = sync::clone::classify(locals, remotes)?;
        Ok((actions, state, state_path))
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

    /// DELETE a pointer file (`point`, §5.4): absence is a normal outcome.
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
    /// No name is special: the transfer order follows the user's enumeration verbatim.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Enumerate`] when the enumeration is empty, the first [`Verdict`](crate::error::Verdict) refusal of the mirror diff, [`Error::Mismatch`] on a staged-body divergence, [`Error::Misuse`] if a task panicked, and [`Error::Io`] when the staging directory cannot be managed.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn mirror(&self, dst: &Nxr, enum_src: Enumeration) -> Result<Summary, Error> {
        let actions = self.mirror_plan(dst, enum_src).await?;
        // No implicit claim: no name is special, the enumeration order stands.
        let claim = None::<ArtifactName>;
        let staging = mirror::staging_dir(&self.base, &dst.base);
        secure_staging(&staging)?;
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
                    summary.failures.push(Failure {
                        name: n.to_string(),
                        code: DATA_EXIT,
                    });
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

    /// List the entries of any directory URL through the search API
    /// (the `nxr-mount` readdir path).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when `url` is not a directory URL and [`Error::Enumerate`]
    /// when the search endpoint is unavailable or unparseable.
    pub async fn ls_at(&self, url: &str) -> Result<Vec<layout::ls::Entry>, Error> {
        layout::ls::search_entries(&self.client, url).await
    }

    /// Artifact names under this base via the search API (experimental).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when the base is not a directory URL and [`Error::Enumerate`] when the search endpoint is unavailable or unparseable.
    pub async fn ls_assets(&self) -> Result<Vec<ArtifactName>, Error> {
        layout::ls::search_assets(&self.client, &self.base).await
    }

    /// Poll the search index until every given name is visible, or the budget runs out.
    ///
    /// The search index on a real server lags behind completed writes by seconds.
    /// a consumer that reads through search right after `up` sees a stale world.
    /// The poll asks for the full asset list every 500ms and counts how many of
    /// `names` are present. The returned count may be lower than `names.len()`:
    /// the caller decides whether that is a warning or a failure, and the upload
    /// itself has already succeeded either way.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Enumerate`] when the search endpoint is unavailable (the
    /// wait is pointless there: the run cannot confirm visibility at all).
    pub async fn wait_search_visible(
        &self,
        names: &[ArtifactName],
        timeout_secs: u64,
    ) -> Result<usize, Error> {
        if names.is_empty() {
            return Ok(0);
        }
        // The repo name for the search endpoint: the segment after
        // `/repository/` when the base carries one, else the first path
        // segment (the mock and plain static hosts serve the repository
        // from the origin root).
        let url =
            reqwest::Url::parse(&self.base).map_err(|e| Error::misuse(format!("base URL: {e}")))?;
        let segs: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();
        let repo = match segs.iter().position(|s| *s == "repository") {
            Some(i) if i + 1 < segs.len() => segs[i + 1].to_owned(),
            _ => (*segs.first().unwrap_or(&"raw-main")).to_owned(),
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        // Search reports repo-relative paths (`/<group…>/<version>/<name>`).
        // The watched prefix is everything after `/repository/<repo>/`, or the
        // whole path for a root-style base: a last-segment shortcut misses a
        // grouped base (`/g/1.0.0/a.zip` is not `/1.0.0/a.zip`).
        let prefix = match segs.iter().position(|s| *s == "repository") {
            Some(i) => segs
                .get(i + 2..)
                .map(|rest| rest.join("/"))
                .unwrap_or_default(),
            None => segs.join("/"),
        };
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        let mut visible: usize;
        let mut poll_rounds: u32 = 0;
        loop {
            // The full asset page set for the repository, filtered client-side
            // to the watched names: the watch is a visibility check, not an
            // enumeration, so a marker or a foreign path in the answer is noise.
            let present: std::collections::HashSet<String> =
                layout::ls::search_paths(&self.client, &self.base, &repo).await?;
            visible = names
                .iter()
                .filter(|n| present.contains(&format!("/{prefix}{}", n.as_str())))
                .count();
            if visible == names.len() {
                return Ok(visible);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(visible);
            }
            // Exponential backoff: the index lag is seconds, not milliseconds,
            // so hammering the full listing every 500ms buys nothing. 500ms →
            // 1s → 2s → 4s, capped: a 30s budget costs ~8 requests, not ~60.
            let delay = std::cmp::min(
                std::time::Duration::from_millis(500).saturating_mul(1 << poll_rounds.min(4)),
                std::time::Duration::from_secs(5),
            );
            tokio::time::sleep(delay).await;
            poll_rounds += 1;
        }
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
///   which also refuses a symlink plant at the predictable temp name. A dead
///   run's leftover file is taken over once the flock comes free.
/// - windows: the file is opened with read and write sharing denied, so a
///   second open for writing fails while this run lives. Share-delete stays
///   granted, otherwise the clean run's staging cleanup could not unlink the
///   file this very handle still holds.
///
/// # Errors
///
/// Create the mirror staging directory privately, refusing to follow a planted symlink.
///
/// The path is predictable in a shared temp dir: a pre-created symlink (or a
/// foreign-owned directory) is an attack, not resume state.
#[cfg(not(target_arch = "wasm32"))]
fn secure_staging(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                // A symlink is never a run's own directory: drop the link, not its target.
                std::fs::remove_file(path).map_err(|e| Error::io(path, e))?;
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(Error::Misuse(format!(
                    "the mirror staging path is not a directory: {}",
                    path.display()
                )));
            }
            Ok(meta) => {
                // SAFETY: getuid has no preconditions and cannot fail.
                let uid = unsafe { libc::getuid() };
                if meta.uid() != uid {
                    return Err(Error::Misuse(format!(
                        "the mirror staging directory is owned by another user: {}",
                        path.display()
                    )));
                }
                if meta.permissions().mode() & 0o077 != 0 {
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                        .map_err(|e| Error::io(path, e))?;
                }
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(path, e)),
        }
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|e| Error::io(path, e))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path).map_err(|e| Error::io(path, e))
    }
}

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
            Ok(file) => {
                // The creator must hold the lock too: returning the handle
                // unlocked lets a second process open the same file and win
                // `LOCK_EX` while the first run is already writing parts.
                // SAFETY: `file` owns a live descriptor for this call, and
                // `flock` only inspects it.
                let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if rc == 0 {
                    return Ok(file);
                }
                return Err(Error::io(&path, std::io::Error::last_os_error()));
            }
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod staging_lock_tests {
    use super::acquire_staging_lock;

    /// The creator must hold the lock too: a second call on the same fresh dir is refused.
    /// A symlinked staging path is replaced, never followed: the target stays empty.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_staging_path_is_replaced_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::create_dir(&victim).unwrap();
        let staging = dir.path().join("staging");
        std::os::unix::fs::symlink(&victim, &staging).unwrap();

        super::secure_staging(&staging).unwrap();
        let meta = std::fs::symlink_metadata(&staging).unwrap();
        assert!(meta.is_dir(), "a real directory");
        assert!(!meta.file_type().is_symlink(), "the link is gone");
        assert!(
            std::fs::metadata(&victim).unwrap().is_dir(),
            "the target is untouched"
        );
    }

    #[test]
    fn the_creator_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire_staging_lock(dir.path()).expect("the first run takes the lock");
        let second = acquire_staging_lock(dir.path()).expect_err("the second run is refused");
        assert!(
            matches!(second, crate::error::Error::Misuse(_)),
            "{second:?}"
        );
        drop(first);
    }
}
