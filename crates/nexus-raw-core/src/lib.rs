//! The crate docs live in [`protocol.md`](src/protocol.md), included at compile time:
//! this file carries the module tree and the re-exports, nothing else.
#![doc = include_str!("protocol.md")]

pub mod config;
pub mod creds;
pub mod error;
pub mod events;
pub mod layout;
pub mod model;
pub mod nxr;
pub mod primitive;
pub mod service;
pub mod sync;
pub mod transport;

// The flat public surface: wrappers import from the crate root.
pub use crate::config::Config;
pub use crate::error::{Error, Verdict};
pub use crate::events::{Dir, Event, Progress, Summary};
pub use crate::layout::{ChannelOutcome, ClearOutcome, Manifest};
pub use crate::model::digest::Digest;
pub use crate::model::name::{ArtifactName, NamePrefix};
pub use crate::model::state::{LocalStatus, RemoteStatus};
pub use crate::nxr::{Enumeration, Nxr};
pub use crate::primitive::{GetOutcome, ShaSource};
pub use crate::sync::mirror::{staging_dir, MirrorAction, VERSION_DOCUMENT};
pub use crate::sync::{Action, Mode, RmAction};
pub use crate::transport::client::{DeleteOutcome, HeadInfo, NexusClient};
