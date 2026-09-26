//! # nexus-raw-core
//!
//! The core of nexus-raw: curl for a Sonatype Nexus raw repository.
//! The entry point is the [`Nxr`] facade.
//! The CLI and wrappers are thin and carry no protocol logic.
//!
//! ## Layers
//!
//! - **L0 transport + primitives**: [`transport`] (retries, stall, TLS, auth)
//!   and [`primitive`] (`get`/`put`/`head`/`sha`), curl-grade, no verification.
//! - **L1 transfer**: [`sync`], directory up/down with the symmetric diff,
//!   sha-sibling markers, parallel workers and Range-resume (`.part` files).
//! - **L2 layout helpers**: [`layout`], channels (token refs with any name),
//!   manifests (the enumeration source for down), search-based listings.
//! - **L3 UX** lives in the CLI: doctor, logs, hints, NDJSON.
//!
//! ## The shape of a store
//!
//! ```text
//! <base>/<name>             artifact bytes
//! <base>/<name>.sha256      sha-sibling: "<hex>  <name>\n" (sha256sum -c)
//! <base>/manifest.json      conventional name list (enumeration for down)
//! <channel-url>             a token file: "<token>\n", any name
//! ```
//!
//! A name is complete = bytes + sibling with a matching digest.
//! Local/remote states: `Complete`, `Markerless` (bytes without a marker),
//! `Broken` (marker mismatches or does not parse), `Absent`.
//! The symmetric diff decides what to transfer.
//! `Mismatch` and `Missing` refuse without overwriting.
//!
//! `up` writes markers by default; `--no-sha` opts out.
//! `down` requires an enumeration source: a manifest, explicit names,
//! or the best-effort search API.
//!
//! Errors: [`Error`] with [`Error::exit_code`] (0 ok, 1 data, 2 misuse,
//! 3 transport) and [`Error::hint`], the human hint the CLI renders.

pub mod config;
pub mod creds;
pub mod error;
pub mod events;
pub mod layout;
pub mod model;
pub mod nxr;
pub mod primitive;
pub mod sync;
pub mod transport;

// The flat public surface: wrappers import from the crate root.
pub use crate::config::Config;
pub use crate::error::{Error, Verdict};
pub use crate::events::{Dir, Event, Progress, Summary};
pub use crate::layout::{ChannelOutcome, Manifest};
pub use crate::model::digest::Digest;
pub use crate::model::name::ArtifactName;
pub use crate::model::state::{LocalStatus, RemoteStatus};
pub use crate::nxr::{Enumeration, Nxr};
pub use crate::primitive::{GetOutcome, ShaSource};
pub use crate::sync::{Action, Mode};
pub use crate::transport::client::HeadInfo;
pub use crate::transport::client::NexusClient;
