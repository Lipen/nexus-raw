//! Node bindings for nexus-raw: the `nxr` CLI surface as promise-returning functions.
//!
//! Built on napi-rs 3: async exports run on napi's built-in multi-threaded tokio runtime (the `async` feature with all drivers enabled).
//! Events cross the boundary as parsed JSON (the `serde-json` feature).
//!
//! Every command is one self-sufficient call, exactly like the CLI: the base URL is in argv, credentials come from `auth` or the env fallback (`NXR_AUTH`, then `NXR_USERNAME` + `NXR_PASSWORD`), no config file.
//! A promise resolves to the command's result and rejects with an `Error` whose `exitCode` and `hint` mirror `Error::exit_code` and `Error::hint` of the core.
//! The JS entry (`index.js`) promotes them from the rejection message the raw addon produces.
//!
//! Platform status: `linux-x86_64-gnu`, `darwin-x64` and `darwin-arm64` ship as prebuilt packages.
//! The win32 addon waits on an npm ticket; there the loader falls back to a local build.

mod error;
mod input;
mod layout;
mod mapping;
mod opts;
mod primitives;
mod pump;
mod result;
mod transfer;

pub use layout::*;
pub use opts::*;
pub use primitives::*;
pub use result::*;
pub use transfer::*;

/// The unit-test build links without a Node process (the noop feature makes the napi runtime a no-op), but the noop feature does not cover the threadsafe-function sys calls.
/// The tests never carry an event callback and the test pump forgets it, so these stubs only satisfy the linker; nothing reaches them.
#[cfg(test)]
mod node_stubs {
    #[no_mangle]
    extern "C" fn napi_release_threadsafe_function(
        _func: *mut std::ffi::c_void,
        _mode: std::ffi::c_int,
    ) -> std::ffi::c_int {
        0
    }
}
