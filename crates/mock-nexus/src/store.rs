//! Shared mock state: the object store, the request log and the per-scenario knobs.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::server::Drip;
use crate::{MockNexus, Scenario};

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
    /// Member addresses of a group instance, in member order.
    /// Empty for a hosted instance: a group is a separate deployment kind, not a scenario.
    pub(crate) members: Vec<SocketAddr>,
}

impl Shared {
    /// Shared state for a hosted instance running `scenario`: an empty store plus every scenario-derived knob.
    pub(crate) fn hosted(scenario: Scenario) -> Self {
        let auth_b64 = match &scenario {
            Scenario::Auth401 { user, pass } => {
                Some(crate::base64::encode(format!("{user}:{pass}").as_bytes()))
            }
            _ => None,
        };
        let drip = match &scenario {
            Scenario::Slow {
                chunk_delay_ms,
                chunk_size,
            } => Some(Drip {
                delay_ms: *chunk_delay_ms,
                chunk_size: *chunk_size,
            }),
            _ => None,
        };
        let sizeless = matches!(&scenario, Scenario::Sizeless);
        let (partial_first_put, flaky_first) = match &scenario {
            Scenario::PartialPut {
                first_attempt_bytes,
            } => (Some(*first_attempt_bytes), None),
            Scenario::Flaky { first_failures } => (None, Some(*first_failures)),
            _ => (None, None),
        };
        Self {
            scenario,
            store: Mutex::new(HashMap::new()),
            log: Mutex::new(Vec::new()),
            first_request: Mutex::new(HashMap::new()),
            first_put: Mutex::new(HashMap::new()),
            drift: AtomicBool::new(false),
            auth_b64,
            drip,
            sizeless,
            partial_first_put,
            flaky_first,
            members: Vec::new(),
        }
    }

    /// Shared state for a group over `members`: a plain hosted base plus the member addresses, in member order.
    pub(crate) fn group(members: &[&MockNexus]) -> Self {
        Self {
            members: members.iter().map(|m| m.addr()).collect(),
            ..Self::hosted(Scenario::Atomic)
        }
    }
}

/// Lock a mutex, surviving poisoning: a panicking test thread must not take unrelated assertions down with it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
