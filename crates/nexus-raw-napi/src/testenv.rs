//! Test-only environment guard for the credential variables.
//!
//! `build_config` reads `NXR_AUTH`, `NXR_USERNAME` and `NXR_PASSWORD` when a call
//! carries no explicit `auth`. Tests run in parallel threads over one process
//! environment, so a test that mutates those variables and a test that reads
//! them must not overlap: every such test holds [`ENV_LOCK`] and clears the
//! variables for its lifetime through [`CleanEnv`].

use std::sync::Mutex;

/// One lock for the whole crate: a test that writes the credential variables
/// and a test that reads them never run concurrently.
pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Removes the credential variables for its lifetime and restores them on drop,
/// so a developer's exported values cannot decide a test that asserts defaults.
pub(crate) struct CleanEnv {
    saved: Vec<(&'static str, Option<String>)>,
}

impl CleanEnv {
    pub(crate) fn new() -> Self {
        const VARS: [&str; 4] = ["NXR_AUTH", "NXR_USERNAME", "NXR_PASSWORD", "NXR_CONFIG"];
        let saved = VARS.iter().map(|v| (*v, std::env::var(v).ok())).collect();
        for v in VARS {
            std::env::remove_var(v);
        }
        Self { saved }
    }
}

impl Drop for CleanEnv {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}
