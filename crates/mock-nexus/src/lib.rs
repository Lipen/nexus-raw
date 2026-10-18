//! Mock Nexus raw-storage server with a failure-scenario table.
//!
//! Implements just enough HTTP/1.1 (std only) to exercise the nexus-raw transport contract.
//! `GET`/`HEAD`/`PUT` with `Content-Length` or chunked bodies, `DELETE` with 204/404.
//! Percent-encoded paths stored verbatim.
//! One request per connection, `Connection: close` on every response.
//! GET honors resumable downloads: a single open `Range: bytes=N-` is answered with `206` and `Content-Range`, out-of-range starts get `416`.
//! Any other `Range` form is ignored.
//! A [`Scenario`] selects a failure mode: truncated PUT and GET bodies, connection resets, held uploads, missing `Content-Length`, slow links, drifted documents, flaky 503s, 503s with a `Retry-After` pause, rate-limit 429s with `Retry-After`, redirects, and Basic-auth gating.
//!
//! Rust conformance tests use the library API directly.
//! The `mock-nexus` binary exposes the same scenarios to shell- and Python-driven tests:
//!
//! ```text
//! let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::Atomic)?;
//! let url = format!("{}1.14.0/version.json", server.base_url());
//! ```
//!
//! Presets are named points in a behavior space; a [`Spec`] composes the space itself:
//! layers stack (`Spec::atomic().with_auth(..).with_flaky(3)`), and `MockNexus::start_spec`
//! runs a composed instance. A composed instance never enters the contract tables:
//! presets and their drift guards are the contract, specs are code-level composition.
//!
//! A group repository is a separate deployment kind, not a scenario:
//! [`MockNexus::start_group`] aggregates two or more running members, forwarding reads in member order and refusing writes.
//! On a group handle the store mutators ([`MockNexus::insert`], [`MockNexus::enable_drift`]) are no-ops: a group stores nothing.
//!
//! The client-facing contract page `docs/reference/invariants.md` is rendered from [`SCENARIO_DOCS`].
//! A drift test in the `invariants` module fails when a scenario change does not regenerate it.

mod base64;
mod group;
mod handle;
mod http;
mod invariants;
mod scenario;
mod server;
mod service_api;
mod spec;
mod storage;
mod store;
mod support;
#[cfg(test)]
mod tests;

pub use handle::MockNexus;
pub use invariants::{render_invariants_markdown, ScenarioDoc, SCENARIO_DOCS};
pub use scenario::{Scenario, SCENARIOS};
pub use server::Drip;
pub use spec::Spec;
pub use store::{Outcome, ReqLog};
