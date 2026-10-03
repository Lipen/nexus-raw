//! The `Nxr` facade: the single entry point for CLI and wrappers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc, Semaphore};

use crate::config::Config;
use crate::error::Error;
use crate::events::{Dir, Event, Progress, Summary};
use crate::layout::{self, Manifest};
use crate::model::digest::Digest;
use crate::model::name::ArtifactName;
use crate::model::state::{LocalStatus, RemoteStatus};
use crate::primitive::{self, GetOutcome, HeadInfo, ShaSource};
use crate::sync::{self, down, rm::RmAction, up, Action, Mode};
use crate::transport::client::NexusClient;

/// How `down` learns which names to fetch (spec §5.2.1).
#[derive(Debug, Clone)]
pub enum Enumeration {
    /// A parsed manifest (`--manifest`).
    Manifest(Manifest),
    /// Explicit names (`--name`, repeatable).
    Names(Vec<ArtifactName>),
    /// Best-effort search-API traversal (`--ls`).
    Search,
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
    pub fn base(&self) -> &str {
        &self.base
    }

    // ---- L0: primitives -------------------------------------------------

    /// GET a URL (spec §5.1).
    pub async fn get(
        &self,
        url: &str,
        out: Option<PathBuf>,
        cont: bool,
    ) -> Result<GetOutcome, Error> {
        primitive::get(&self.client, url, out, cont).await
    }

    /// PUT a file, optionally with its sha-sibling (spec §5.1).
    pub async fn put(
        &self,
        url: &str,
        src: &Path,
        sha: bool,
    ) -> Result<(u64, Option<Digest>), Error> {
        primitive::put(&self.client, url, src, sha).await
    }

    /// HEAD a URL (spec §5.1).
    pub async fn head(&self, url: &str) -> Result<HeadInfo, Error> {
        primitive::head(&self.client, url).await
    }

    /// The sha256 of a file or URL (spec §5.1).
    pub async fn sha(&self, src: ShaSource) -> Result<Digest, Error> {
        primitive::sha(&self.client, src).await
    }

    // ---- L1: transfer ----------------------------------------------------

    /// The artifact names of a local directory (plain-mode scan, §5.2).
    pub fn scan(&self, dir: &Path) -> Result<Vec<ArtifactName>, Error> {
        sync::scan_dir(dir)
    }

    /// The symmetric plan without transferring anything (`up --dry-run`).
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

    /// Upload a local directory (§5.2).
    ///
    /// `names` restricts the transfer (manifest mode).
    /// `None` scans the directory.
    /// Markers are written by default.
    /// `gen_markers == false` (`--no-sha`) uploads bytes only.
    /// `claim` names the file that must land before any other byte does (claim-first publishing, §2): it uploads alone, and a failed claim aborts the run without starting the rest.
    pub async fn up(
        &self,
        dir: &Path,
        names: Option<Vec<ArtifactName>>,
        gen_markers: bool,
        claim: Option<ArtifactName>,
        plan: Option<Vec<Action>>,
    ) -> Result<Summary, Error> {
        if !dir.is_dir() {
            return Err(Error::misuse(format!("not a directory: {}", dir.display())));
        }
        let names = match names {
            Some(list) => {
                let scanned = self.scan(dir)?;
                let have: std::collections::HashSet<&ArtifactName> = scanned.iter().collect();
                let absent: Vec<String> = list
                    .iter()
                    .filter(|n| !have.contains(n))
                    .map(|n| n.to_string())
                    .collect();
                if !absent.is_empty() {
                    return Err(Error::Missing { names: absent });
                }
                list
            }
            None => {
                let scanned = self.scan(dir)?;
                if scanned.is_empty() {
                    return Err(Error::misuse(format!(
                        "nothing to upload: {} holds no artifacts",
                        dir.display()
                    )));
                }
                scanned
            }
        };
        let mut locals = sync::local_statuses(dir, names).await;
        if gen_markers {
            generate_markers(dir, &mut locals).await?;
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
        let actions = match plan {
            Some(p) => p,
            None => {
                let remotes = self
                    .remote_states_for(locals.iter().map(|(n, _)| n))
                    .await?;
                let d = dir.to_owned();
                let markers = gen_markers;
                tokio::task::spawn_blocking(move || {
                    sync::classify(&d, Mode::Up, markers, locals, remotes)
                })
                .await
                .map_err(|e| Error::misuse(format!("task panicked: {e}")))?
                .map_err(Error::from)?
            }
        };
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
    pub async fn down(
        &self,
        dir: &Path,
        enum_src: Enumeration,
        fresh: bool,
        plan: Option<Vec<Action>>,
    ) -> Result<Summary, Error> {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| Error::io(dir, e))?;
        down::cleanup_orphans(dir)?;
        let names = match enum_src {
            Enumeration::Manifest(m) => m.names,
            Enumeration::Names(v) => v,
            Enumeration::Search => layout::ls::search_assets(&self.client, &self.base).await?,
        };
        if names.is_empty() {
            return Err(Error::Enumerate {
                url: self.base.clone(),
                reason: "enumeration produced no names; the URL is probably wrong".into(),
            });
        }
        let actions = match plan {
            Some(p) => p,
            None => {
                let locals = sync::local_statuses(dir, names).await;
                let remotes = self
                    .remote_states_for(locals.iter().map(|(n, _)| n))
                    .await?;
                let d = dir.to_owned();
                tokio::task::spawn_blocking(move || {
                    sync::classify(&d, Mode::Down, true, locals, remotes)
                })
                .await
                .map_err(|e| Error::misuse(format!("task panicked: {e}")))?
                .map_err(Error::from)?
            }
        };
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
    pub async fn rm(&self, enum_src: Enumeration) -> Result<Summary, Error> {
        let names = self.rm_names(enum_src).await?;
        sync::rm::execute(self.client.clone(), self.base.clone(), names).await
    }

    /// The deletion plan without deleting anything (`rm --dry-run`).
    ///
    /// Existence only: a diverging or broken remote copy still plans as `Remove`,
    /// because deletion is about names, never about content.
    pub async fn rm_plan(&self, enum_src: Enumeration) -> Result<Vec<RmAction>, Error> {
        let names = self.rm_names(enum_src).await?;
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
    async fn rm_names(&self, enum_src: Enumeration) -> Result<Vec<ArtifactName>, Error> {
        let names = match enum_src {
            Enumeration::Manifest(m) => m.names,
            Enumeration::Names(v) => v,
            Enumeration::Search => layout::ls::search_assets(&self.client, &self.base).await?,
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
    pub async fn point_clear(&self, url: &str) -> Result<layout::ClearOutcome, Error> {
        layout::pointer_clear(&self.client, url).await
    }

    /// Local verification of a directory: bytes + marker + digest, no network.
    /// Emits only the final [`Event::Summary`].
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
    pub async fn channel_get(&self, url: &str) -> Result<Option<String>, Error> {
        layout::channel_get(&self.client, url).await
    }

    /// Write a channel ref with the optional forward-only guard.
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
    pub async fn manifest_at_base(&self) -> Result<Option<Manifest>, Error> {
        let url = format!("{}manifest.json", self.base);
        match self.client.get_small(&url).await? {
            None => Ok(None),
            Some(bytes) => Manifest::from_slice(&bytes).map(Some),
        }
    }

    /// Fetch a manifest from an arbitrary URL (`--manifest <url>`).
    pub async fn manifest_from(&self, url: &str) -> Result<Manifest, Error> {
        Manifest::from_url(&self.client, url).await
    }

    /// Versions under this base via the search API (experimental).
    pub async fn ls_versions(&self) -> Result<Vec<String>, Error> {
        layout::ls::search_versions(&self.client, &self.base).await
    }

    /// Artifact names under this base via the search API (experimental).
    pub async fn ls_assets(&self) -> Result<Vec<ArtifactName>, Error> {
        layout::ls::search_assets(&self.client, &self.base).await
    }

    // ---- internals ---------------------------------------------------------

    /// Remote states for the given names, in order.
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

/// Give every markerless local file its sibling: hash the bytes, write the canonical marker (§5.2 "markers are mandatory where we write").
/// Broken markers refuse early.
/// The file is never touched.
async fn generate_markers(
    dir: &Path,
    locals: &mut [(ArtifactName, LocalStatus)],
) -> Result<(), Error> {
    let dir = dir.to_owned();
    let mut jobs: Vec<(ArtifactName, LocalStatus)> = Vec::new();
    for (name, st) in locals.iter_mut() {
        match st {
            LocalStatus::Broken(detail) => {
                return Err(Error::Mismatch {
                    name: name.to_string(),
                    detail: format!("local object is broken and must not be overwritten: {detail}"),
                })
            }
            LocalStatus::Markerless => jobs.push((name.clone(), st.clone())),
            _ => {}
        }
    }
    if jobs.is_empty() {
        return Ok(());
    }
    let made = tokio::task::spawn_blocking(move || {
        let mut made: Vec<(ArtifactName, crate::model::digest::Digest)> = Vec::new();
        for (name, _) in jobs {
            let bytes = crate::model::state::bytes_path(&dir, &name);
            let d = crate::model::digest::sha256_file(&bytes).map_err(|e| Error::io(&bytes, e))?;
            let sib = crate::model::state::sibling_path(&dir, &name);
            // The marker path derives from the object name: never follow a symlink planted there (same rule as the download side).
            let mut f = crate::transport::client::write_options_blocking(false)
                .open(&sib)
                .map_err(|e| Error::io(&sib, e))?;
            use std::io::Write as _;
            f.write_all(crate::model::sibling::format_line(name.as_str(), &d).as_bytes())
                .map_err(|e| Error::io(&sib, e))?;
            made.push((name, d));
        }
        Ok::<Vec<_>, Error>(made)
    })
    .await
    .map_err(|e| Error::misuse(format!("task panicked: {e}")))??;
    for (name, st) in locals.iter_mut() {
        if let Some((_, d)) = made.iter().find(|(n, _)| n == name) {
            *st = LocalStatus::Complete(d.clone());
        }
    }
    Ok(())
}
