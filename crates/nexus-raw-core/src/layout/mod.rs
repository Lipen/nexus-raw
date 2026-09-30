//! L2 layout helpers: channels, manifests, listings.
//! Conventions built on the L0/L1 primitives.
//! Nothing here is protocol.

pub mod channel;
pub mod ls;
pub mod manifest;

pub use channel::{get as channel_get, set as channel_set, ChannelOutcome};
pub use manifest::Manifest;
