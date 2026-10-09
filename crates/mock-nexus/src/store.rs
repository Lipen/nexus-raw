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
    /// GET-success counter per path (cut-body gate).
    pub(crate) first_get: Mutex<HashMap<String, u32>>,
    pub(crate) drift: AtomicBool,
    /// Expected `Authorization` token when the scenario requires Basic auth.
    pub(crate) auth_b64: Option<String>,
    /// Whether the EULA gate has been opened (eula-gate scenario only).
    pub(crate) eula_accepted: Mutex<bool>,
    /// Slow-drip parameters for GET/HEAD bodies.
    pub(crate) drip: Option<Drip>,
    /// Success GET/HEAD answers hide `Content-Length` (the proxy case).
    pub(crate) sizeless: bool,
    /// Bytes served of the first PUT per path before cutting it (partial-put).
    pub(crate) partial_first_put: Option<usize>,
    /// Bytes served of the first success GET per path before cutting it, with the `Content-Length` lie flag (cut-body).
    pub(crate) cut_first_get: Option<(usize, bool)>,
    /// Number of initial 429s per path with their `Retry-After` seconds (rate-limit).
    pub(crate) rate_first: Option<(usize, u64)>,
    /// Number of initial 503s per path (flaky).
    pub(crate) flaky_first: Option<u32>,
    /// Member addresses of a group instance, in member order.
    /// Empty for a hosted instance: a group is a separate deployment kind, not a scenario.
    pub(crate) members: Vec<SocketAddr>,
}

impl Shared {
    /// Shared state for a hosted instance running `scenario`: an empty store plus every scenario-derived knob.
    pub(crate) fn hosted(scenario: Scenario, base_url: String) -> Self {
        let auth_b64 = match &scenario {
            Scenario::Auth401 { user, pass } | Scenario::Auth403 { user, pass } => {
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
        let (partial_first_put, cut_first_get, rate_first, flaky_first) = match &scenario {
            Scenario::PartialPut {
                first_attempt_bytes,
            } => (Some(*first_attempt_bytes), None, None, None),
            Scenario::CutBody {
                after_bytes,
                fake_length,
            } => (None, Some((*after_bytes, *fake_length)), None, None),
            Scenario::RateLimit {
                first_429s,
                retry_after_secs,
            } => (None, None, Some((*first_429s, *retry_after_secs)), None),
            Scenario::Flaky { first_failures } => (None, None, None, Some(*first_failures)),
            _ => (None, None, None, None),
        };
        Self {
            store: Mutex::new(initial_store(&scenario, &base_url)),
            scenario,
            log: Mutex::new(Vec::new()),
            first_request: Mutex::new(HashMap::new()),
            first_put: Mutex::new(HashMap::new()),
            first_get: Mutex::new(HashMap::new()),
            drift: AtomicBool::new(false),
            auth_b64,
            eula_accepted: Mutex::new(false),
            drip,
            sizeless,
            partial_first_put,
            cut_first_get,
            rate_first,
            flaky_first,
            members: Vec::new(),
        }
    }

    /// Shared state for a group over `members`: a plain hosted base plus the member addresses, in member order.
    /// The group's own service document is irrelevant: reads forward to the members, whose stores carry it.
    pub(crate) fn group(members: &[&MockNexus]) -> Self {
        Self {
            members: members.iter().map(|m| m.addr()).collect(),
            ..Self::hosted(Scenario::Atomic, String::new())
        }
    }
}

/// Lock a mutex, surviving poisoning: a panicking test thread must not take unrelated assertions down with it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The store a hosted instance starts with: empty, except the server metadata
/// the service REST API reports (the repositories document).
/// A [`Scenario::NoService`] instance predates the management API: nothing is seeded.
fn initial_store(scenario: &Scenario, base_url: &str) -> HashMap<String, Vec<u8>> {
    let mut store = HashMap::new();
    if matches!(scenario, Scenario::NoService) {
        return store;
    }
    let doc = format!(
        concat!(
            "[",
            r#"{{"name":"raw-main","format":"raw","type":"hosted","url":"{base}repository/raw-main/","attributes":{{}}}}"#,
            ",",
            r#"{{"name":"raw-all","format":"raw","type":"group","url":"{base}repository/raw-all/","attributes":{{}}}}"#,
            "]"
        ),
        base = base_url
    );
    store.insert("service/rest/v1/repositories".to_owned(), doc.into_bytes());
    match scenario {
        Scenario::ServiceStatusEmpty => {
            store.insert("service/rest/v1/status".to_owned(), Vec::new());
            store.insert("service/rest/v1/status/writable".to_owned(), Vec::new());
        }
        Scenario::ServiceStatusVersion => {
            store.insert(
                "service/rest/v1/status".to_owned(),
                br#"{"version": "3.79.1-04"}"#.to_vec(),
            );
            store.insert("service/rest/v1/status/writable".to_owned(), Vec::new());
        }
        Scenario::RepoCollectionTrimmed => {
            let rich = format!(
                "[{{\"name\":\"raw-main\",\"format\":\"raw\",\"type\":\"hosted\",\"url\":\"{base}repository/raw-main/\",\"size\":42,\"attributes\":{{\"storage\":{{\"blobStoreName\":\"default\"}}}}}},{{\"name\":\"raw-all\",\"format\":\"raw\",\"type\":\"group\",\"url\":\"{base}repository/raw-all/\",\"size\":7,\"attributes\":{{}}}}]",
                base = base_url
            );
            store.insert("service/rest/v1/repositories".to_owned(), rich.into_bytes());
        }
        _ => {}
    }
    store
}
