//! The `Nxr` facade: the single entry point for CLI and wrappers.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::{mpsc, Semaphore};

use crate::config::Config;
use crate::diff::{self, Action};
use crate::error::Error;
use crate::events::{Dir, Event, Progress, Summary};
use crate::model::claim::Claim;
use crate::model::name::{self, ArtifactName};
use crate::model::state::{LocalStatus, RemoteStatus};
use crate::ops::{down, pointer, up};
use crate::transport::client::NexusClient;

/// Facade over the transport and the operation executors.
pub struct Nxr {
    client: Arc<NexusClient>,
    workers: Arc<Semaphore>,
    events: Progress,
}

impl Nxr {
    pub fn new(cfg: Config, events: mpsc::UnboundedSender<Event>) -> Result<Self, Error> {
        let progress = Progress::new(events);
        let client = Arc::new(NexusClient::new(&cfg, progress.clone())?);
        let workers = client.workers();
        Ok(Self {
            client,
            workers,
            events: progress,
        })
    }

    /// The remote claim of a version: None on 404; a broken claim is [`Error::ClaimDrift`].
    pub async fn read_remote_claim(&self, version: &str) -> Result<Option<Claim>, Error> {
        name::validate_version(version)?;
        self.client.get_claim(version).await
    }

    /// The pointer's value: None on 404.
    pub async fn read_pointer(&self, pointer: &str) -> Result<Option<String>, Error> {
        if !name::is_pointer_name(pointer) {
            return Err(Error::misuse(format!(
                "pointer name must be one of latest|nightly, got {pointer:?}"
            )));
        }
        self.client.get_pointer(pointer).await
    }

    /// The pointer's resolved version token: the trailing newline is stripped,
    /// a 404 is an [`Error::Http`], an unparseable pointer file is a data error.
    pub async fn resolve_pointer(&self, pointer: &str) -> Result<String, Error> {
        let url = self.client.pointer_url(pointer);
        match self.read_pointer(pointer).await? {
            None => Err(Error::Http { status: 404, url }),
            Some(raw) => crate::model::pointer::parse_token(&raw).map_err(|e| Error::Mismatch {
                name: pointer.to_owned(),
                detail: format!("pointer file does not parse: {e}"),
            }),
        }
    }

    /// Remote states of the given names, in `names` order.
    pub async fn remote_states(
        &self,
        version: &str,
        names: &[ArtifactName],
    ) -> Result<Vec<(ArtifactName, RemoteStatus)>, Error> {
        name::validate_version(version)?;
        let mut set = tokio::task::JoinSet::new();
        for (i, n) in names.iter().enumerate() {
            let permit = self
                .workers
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| Error::misuse(format!("worker semaphore closed: {e}")))?;
            let client = self.client.clone();
            let version = version.to_owned();
            let n = n.clone();
            set.spawn(async move {
                let _permit = permit;
                (i, client.probe(&version, &n).await.map(|s| (n, s)))
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

    /// Remote states of the version's claim names (for `ls`).
    pub async fn remote_state(
        &self,
        version: &str,
    ) -> Result<Vec<(ArtifactName, RemoteStatus)>, Error> {
        let claim = self
            .read_remote_claim(version)
            .await?
            .ok_or_else(|| Error::Http {
                status: 404,
                url: self.client.claim_url(version),
            })?;
        self.remote_states(version, &claim.artifacts).await
    }

    /// The symmetric diff of a local directory against the server (§5.2).
    ///
    /// Diff refusals and transport errors arrive as one [`Error`]:
    /// every [`crate::error::Verdict`] maps to exit code 1.
    pub async fn diff(&self, dir: &Path, claim: &Claim) -> Result<Vec<Action>, Error> {
        let locals = diff::local_statuses(dir, claim).await;
        self.diff_with_locals(dir, claim, locals).await
    }

    async fn diff_with_locals(
        &self,
        dir: &Path,
        claim: &Claim,
        locals: Vec<(ArtifactName, LocalStatus)>,
    ) -> Result<Vec<Action>, Error> {
        let remotes = self
            .remote_states(&claim.version, &claim.artifacts)
            .await?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        Ok(diff::classify(dir, claim, locals, remotes)?)
    }

    /// Publish a version: precheck → diff → claim (drift check) → PUT bytes+markers (§6.1).
    ///
    /// `plan` is a precomputed diff (e.g. rerun after a break); `None` diffs inside.
    pub async fn up(
        &self,
        dir: &Path,
        claim: &Claim,
        plan: Option<Vec<Action>>,
    ) -> Result<Summary, Error> {
        if !dir.is_dir() {
            return Err(Error::misuse(format!("not a directory: {}", dir.display())));
        }
        // Up requires Complete for every name: Markerless/Broken/Absent is a build error (§5.2).
        let locals = diff::local_statuses(dir, claim).await;
        let incomplete: Vec<String> = locals
            .iter()
            .filter(|(_, st)| !matches!(st, LocalStatus::Complete(_)))
            .map(|(n, _)| n.to_string())
            .collect();
        if !incomplete.is_empty() {
            return Err(Error::Incomplete { names: incomplete });
        }
        let actions = match plan {
            Some(p) => p,
            None => self.diff_with_locals(dir, claim, locals).await?,
        };
        self.events.plan(&actions, Dir::Up);
        self.client
            .put_claim_checked(&claim.version, &claim.to_bytes())
            .await?;
        up::execute(
            self.client.clone(),
            dir.to_owned(),
            claim.version.clone(),
            actions,
            self.workers.clone(),
        )
        .await
    }

    /// Download a version into a directory: diff → tmp+hash → rename → marker (§6.2).
    ///
    /// `only` restricts the name set; `plan` is a precomputed diff, as in [`Nxr::up`].
    pub async fn down(
        &self,
        dir: &Path,
        claim: &Claim,
        only: Option<&[ArtifactName]>,
        plan: Option<Vec<Action>>,
    ) -> Result<Summary, Error> {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| Error::io(dir, e))?;
        down::cleanup_orphans(dir)?;
        let actions = match plan {
            Some(p) => p,
            None => self.diff(dir, claim).await?,
        };
        let actions = match only {
            None => actions,
            Some(want) => {
                let set: std::collections::HashSet<&ArtifactName> = want.iter().collect();
                actions
                    .into_iter()
                    .filter(|a| match a {
                        Action::Download { name, .. }
                        | Action::Skip { name, .. }
                        | Action::Upload { name, .. } => set.contains(name),
                    })
                    .collect()
            }
        };
        self.events.plan(&actions, Dir::Down);
        down::execute(
            self.client.clone(),
            dir.to_owned(),
            claim.version.clone(),
            actions,
            self.workers.clone(),
        )
        .await
    }

    /// Local verification of a version: bytes + marker + digest, no network.
    /// Emits only the final [`Event::Summary`].
    pub async fn verify(&self, dir: &Path, claim: &Claim) -> Result<Summary, Error> {
        let locals = diff::local_statuses(dir, claim).await;
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

    /// PUT a pointer; `if_newer` is forward-only (§6.3).
    pub async fn point(
        &self,
        pointer: &str,
        version: &str,
        if_newer: bool,
    ) -> Result<pointer::PointOutcome, Error> {
        pointer::point(&self.client, pointer, version, if_newer).await
    }

    /// Version list via REST search (experimental; endpoint depends on the Nexus release).
    pub async fn ls_versions(&self) -> Result<Vec<String>, Error> {
        self.client.search_versions().await
    }
}
