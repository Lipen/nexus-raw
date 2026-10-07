use napi_derive::napi;
use nexus_raw_core::{Delta, RmAction, Side, Summary};

// ---- Node-facing result objects -------------------------------------------

/// The final summary of a transfer or verification.
#[napi(object)]
pub struct NxrSummary {
    pub uploaded: u32,
    pub downloaded: u32,
    pub skipped: u32,
    /// Names the call deleted (`rm`); transfers never delete and stay at 0.
    pub removed: u32,
    pub failed: Vec<String>,
}

impl From<&Summary> for NxrSummary {
    fn from(s: &Summary) -> Self {
        Self {
            uploaded: u32::try_from(s.uploaded).unwrap_or(u32::MAX),
            downloaded: u32::try_from(s.downloaded).unwrap_or(u32::MAX),
            skipped: u32::try_from(s.skipped).unwrap_or(u32::MAX),
            removed: u32::try_from(s.removed).unwrap_or(u32::MAX),
            failed: s.failed.clone(),
        }
    }
}

/// One dry-run action, shaped like the CLI `--json` dry-run lines.
#[napi(object)]
pub struct NxrPlanAction {
    /// `skip`, `upload` or `download`.
    pub action: String,
    pub name: String,
    /// Byte size when the action transfers bytes.
    pub size: Option<f64>,
}

/// The plan a dry run resolves to.
#[napi(object)]
pub struct NxrPlan {
    pub actions: Vec<NxrPlanAction>,
}

/// One planned deletion, shaped like the CLI `rm --dry-run` lines.
#[napi(object)]
pub struct NxrRmPlanAction {
    /// `rm` when the remote copy exists and would be deleted, `missing` when it is already absent.
    pub action: String,
    pub name: String,
    /// Byte size when the remote copy exists.
    pub size: Option<f64>,
}

impl From<&RmAction> for NxrRmPlanAction {
    fn from(a: &RmAction) -> Self {
        match a {
            RmAction::Remove { name, size } => Self {
                action: "rm".into(),
                name: name.to_string(),
                size: size.map(|s| s as f64),
            },
            RmAction::Missing { name } => Self {
                action: "missing".into(),
                name: name.to_string(),
                size: None,
            },
        }
    }
}

/// The plan an `rm` dry run resolves to.
#[napi(object)]
pub struct NxrRmPlan {
    pub actions: Vec<NxrRmPlanAction>,
}

impl From<&nexus_raw_core::Action> for NxrPlanAction {
    fn from(a: &nexus_raw_core::Action) -> Self {
        match a {
            nexus_raw_core::Action::Skip { name, .. } => Self {
                action: "skip".into(),
                name: name.to_string(),
                size: None,
            },
            nexus_raw_core::Action::Upload { name, size, .. } => Self {
                action: "upload".into(),
                name: name.to_string(),
                size: Some(*size as f64),
            },
            nexus_raw_core::Action::Download { name, size, .. } => Self {
                action: "download".into(),
                name: name.to_string(),
                size: size.map(|s| s as f64),
            },
        }
    }
}

/// The content facts one side holds for a name, the CLI `diff --json` side shape.
/// `null` means the fact is unknown on this side, never that it differs.
#[napi(object)]
pub struct NxrDeltaSide {
    /// The sha256 digest, when the side carries one.
    pub digest: Option<String>,
    /// Byte size, when the side reports one.
    pub size: Option<f64>,
}

impl From<&Side> for NxrDeltaSide {
    fn from(s: &Side) -> Self {
        Self {
            digest: s.digest.as_ref().map(|d| d.as_str().to_owned()),
            size: s.size.map(|v| v as f64),
        }
    }
}

/// One delta entry, shaped like the CLI `diff --json` lines.
#[napi(object)]
pub struct NxrDeltaEntry {
    /// `same`, `missing-local`, `missing-remote` or `diverged`.
    pub state: String,
    pub path: String,
    /// The shared digest when the state is `same`.
    pub digest: Option<String>,
    /// The local-side facts when the state is `missing-remote` or `diverged`.
    pub local: Option<NxrDeltaSide>,
    /// The storage-side facts when the state is `missing-local` or `diverged`.
    pub remote: Option<NxrDeltaSide>,
    /// The sizes are known on both sides and differ, `diverged` only.
    pub size: Option<bool>,
    /// The digests are known on both sides and differ, `diverged` only.
    pub sha: Option<bool>,
}

impl From<&Delta> for NxrDeltaEntry {
    fn from(d: &Delta) -> Self {
        match d {
            Delta::Same { name, digest } => Self {
                state: "same".into(),
                path: name.to_string(),
                digest: Some(digest.as_str().to_owned()),
                local: None,
                remote: None,
                size: None,
                sha: None,
            },
            Delta::MissingLocal { name, remote } => Self {
                state: "missing-local".into(),
                path: name.to_string(),
                digest: None,
                local: None,
                remote: Some(NxrDeltaSide::from(remote)),
                size: None,
                sha: None,
            },
            Delta::MissingRemote { name, local } => Self {
                state: "missing-remote".into(),
                path: name.to_string(),
                digest: None,
                local: Some(NxrDeltaSide::from(local)),
                remote: None,
                size: None,
                sha: None,
            },
            Delta::Diverged {
                name,
                local,
                remote,
                size,
                sha,
            } => Self {
                state: "diverged".into(),
                path: name.to_string(),
                digest: None,
                local: Some(NxrDeltaSide::from(local)),
                remote: Some(NxrDeltaSide::from(remote)),
                size: Some(*size),
                sha: Some(*sha),
            },
        }
    }
}

/// The delta report a `diff` resolves to: one entry per name in the union of the local scan and the enumeration, in name order.
#[napi(object)]
pub struct NxrDeltaReport {
    pub entries: Vec<NxrDeltaEntry>,
    /// How many names the report holds.
    pub count: u32,
}

impl From<&[Delta]> for NxrDeltaReport {
    fn from(entries: &[Delta]) -> Self {
        Self {
            entries: entries.iter().map(NxrDeltaEntry::from).collect(),
            count: u32::try_from(entries.len()).unwrap_or(u32::MAX),
        }
    }
}

/// The `get` result.
#[napi(object)]
pub struct NxrGetResult {
    /// Final size in bytes (stdout mode: bytes written).
    pub size: f64,
    /// The digest when a file was produced (stdout streaming does not hash).
    pub sha256: Option<String>,
    /// The offset the transfer resumed from (0 for a fresh download).
    pub resumed_from: f64,
}

/// The `put` result.
#[napi(object)]
pub struct NxrPutResult {
    pub size: f64,
    /// The uploaded digest when the sha-sibling was requested.
    pub sha256: Option<String>,
}

/// The `head` result.
#[napi(object)]
pub struct NxrHeadResult {
    pub status: u32,
    pub size: Option<f64>,
    pub content_type: Option<String>,
}

/// The `channelSet` result.
#[napi(object)]
pub struct NxrChannelSetResult {
    /// False when the forward-only guard kept the current token.
    pub written: bool,
    /// The previous token, when one was readable.
    pub from: Option<String>,
    /// The kept token, when the write was skipped.
    pub current: Option<String>,
}

/// The `pointClear` result, the CLI `--json` shape.
#[napi(object)]
pub struct NxrPointClearResult {
    /// `cleared` when the pointer existed and is deleted, `absent` when it was already gone.
    pub outcome: String,
}
