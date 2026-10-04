use napi_derive::napi;
use nexus_raw_core::{RmAction, Summary};

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
