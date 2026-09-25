//! # nexus-raw-core
//!
//! The core of the nexus-raw protocol: reliable artifact delivery to and from
//! a Sonatype Nexus raw repository.
//! The entry point is the [`Nxr`] facade.
//! The CLI and wrappers are thin and carry no protocol logic.
//!
//! ## The canonical protocol (summary)
//!
//! Layout inside the base URL (`<base>/` is the repository directory, without
//! the version name):
//!
//! ```text
//! <base>/<version>/claim.json          — the version's name list, immutable
//! <base>/<version>/<name>              — artifact bytes
//! <base>/<version>/<name>.sha256       — sha-sibling: "<hex>  <name>\n" (sha256sum -c)
//! <base>/<pointer>                     — pointer: "<token>\n" (latest, nightly)
//! ```
//!
//! A name is complete = bytes + sibling with a matching digest.
//! Local/remote states ([`LocalStatus`] / [`RemoteStatus`]): `Complete`,
//! `Markerless` (bytes without a marker), `Broken` (marker mismatches or does
//! not parse), `Absent`.
//! The symmetric diff (§5.2 of the protocol) decides what to transfer.
//! `Mismatch`, `Missing` and local incompleteness refuse without overwriting.
//!
//! Write order (up): claim (drift check) → then per name, in parallel workers,
//! `PUT <name>` → `PUT <name>.sha256`.
//! The marker strictly follows the bytes of the same name.
//! Down: GET claim → tmp+hash → rename → tmp marker → rename.
//! A partially fetched name leaves neither bytes nor marker at the destination.
//!
//! Transport: GET/HEAD/PUT.
//! Basic auth from env, TLS verified by default.
//! Up to 4 attempts per request with 0.5s×2ⁿ backoff + jitter ≤ 250ms.
//! Stall detection — no bytes for N seconds.
//!
//! Errors: [`Error`] with [`Error::exit_code`] — 0 ok, 1 data, 2 misuse, 3 transport.

pub mod config;
pub mod creds;
pub mod diff;
pub mod error;
pub mod events;
pub mod model;
pub mod nxr;
pub mod ops;
pub mod transport;

// The flat public surface: wrappers import from the crate root.
pub use crate::config::Config;
pub use crate::diff::Action;
pub use crate::error::{Error, Verdict};
pub use crate::events::{Dir, Event, Progress, Summary};
pub use crate::model::claim::Claim;
pub use crate::model::digest::Digest;
pub use crate::model::name::ArtifactName;
pub use crate::model::state::{LocalStatus, RemoteStatus};
pub use crate::nxr::Nxr;
pub use crate::ops::PointOutcome;
