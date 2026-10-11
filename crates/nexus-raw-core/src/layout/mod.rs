//! L2 layout helpers: channels and listings.
//! Conventions built on the L0/L1 primitives.
//! Nothing here is protocol.

pub mod channel;
pub mod ls;

pub use channel::{
    clear as pointer_clear, get as channel_get, set as channel_set, ChannelOutcome, ClearOutcome,
};
