//! Shared mock state: the object store, the request log and the per-scenario knobs.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::server::Drip;
use crate::Scenario;

/// Outcome of one request recorded in the request log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A response with this status code was written.
    Status(u16),
    /// The connection was reset without a response.
    Reset,
    /// The request body ended after `bytes` bytes.
    /// No response was written.
    PartialRead { bytes: usize },
    /// The head was read, then the connection went silent: the body was never read and no response was ever written.
    /// The writer is expected to give up on its own (stall detection).
    Stalled,
}

/// One served request, in arrival order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReqLog {
    pub method: String,
    /// Raw request path: percent-encoding preserved, query stripped, no leading `/` (same key format as [`MockNexus::store_get`](crate::MockNexus::store_get)).
    pub path: String,
    pub outcome: Outcome,
}

/// State shared by the accept loop and every connection thread.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) scenario: Scenario,
    pub(crate) store: Mutex<HashMap<String, Vec<u8>>>,
    pub(crate) log: Mutex<Vec<ReqLog>>,
    /// All-request counter per path (drop-connection, flaky gates).
    pub(crate) first_request: Mutex<HashMap<String, u32>>,
    /// PUT-only counter per path (partial-put gate).
    pub(crate) first_put: Mutex<HashMap<String, u32>>,
    pub(crate) drift: AtomicBool,
    /// Expected `Authorization` token when the scenario requires Basic auth.
    pub(crate) auth_b64: Option<String>,
    /// Slow-drip parameters for GET/HEAD bodies.
    pub(crate) drip: Option<Drip>,
    /// Success GET/HEAD answers hide `Content-Length` (the proxy case).
    pub(crate) sizeless: bool,
    /// Bytes served of the first PUT per path before cutting it (partial-put).
    pub(crate) partial_first_put: Option<usize>,
    /// Number of initial 503s per path (flaky).
    pub(crate) flaky_first: Option<u32>,
}

/// Lock a mutex, surviving poisoning: a panicking test thread must not take unrelated assertions down with it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
