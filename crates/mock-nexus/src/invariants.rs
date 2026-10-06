//! The wire-invariants page: the scenario contract rendered from [`SCENARIO_DOCS`] into `docs/reference/invariants.md`.
//!
//! The page is a checked-in build artifact.
//! A drift test in this module fails when a scenario change does not regenerate it, and `UPDATE_INVARIANTS=1` rewrites it in place.

/// One scenario's row on the wire-invariants page: the id, the human name, the wire behavior and the invariant it pins.
pub struct ScenarioDoc {
    /// The scenario id, exactly the string [`SCENARIOS`] carries.
    pub id: &'static str,
    /// The human name, a sentence.
    pub name: &'static str,
    /// What the server does on the wire, one sentence per element.
    pub behavior: &'static [&'static str],
    /// The wire invariant every client of the protocol can pin on this scenario.
    pub invariant: &'static str,
}

/// The rendered page intro, up to the first scenario heading.
const INTRO: &str = "\
# Wire invariants

This page is the failure-scenario contract for external clients of the protocol.
It lists every scenario the `mock-nexus` fixture serves, what it does on the wire, and the wire invariant each scenario pins.
The content is generated from the scenario table in `crates/mock-nexus/src/scenario.rs` (`SCENARIOS`), the same list `mock-nexus --print-scenarios` prints as JSON.
A scenario added or changed there must land in the same change as this page: the drift test in the `mock-nexus` crate fails until the page is regenerated with `UPDATE_INVARIANTS=1 cargo test -p mock-nexus invariant`.
Run one scenario with `just mock <id>` (or `cargo run -p mock-nexus -- <id>`) and point any client at the printed address.
A group repository is a separate deployment kind, not a scenario.
The per-suite test matrix and the design reasoning: [conformance](../explanation/conformance.md).
";

/// One row per [`SCENARIOS`] entry, in the same order.
/// A unit test in this module pins the alignment.
pub const SCENARIO_DOCS: &[ScenarioDoc] = &[
    ScenarioDoc {
        id: "atomic",
        name: "Straight-through storage, the control group.",
        behavior: &[
            "Every fully-read request is served normally.",
            "PUT stores the bytes and answers `201`, GET serves the stored bytes and HEAD their length, DELETE removes an existing object with `204` and answers `404` on an absent one, and unknown paths answer `404`.",
            "A GET honors resumable downloads: a single open `Range: bytes=N-` is answered with `206` and `Content-Range`, an out-of-range start with `416`.",
        ],
        invariant: "The happy path of the protocol, the baseline every other scenario deviates from on one axis.",
    },
    ScenarioDoc {
        id: "partial-put",
        name: "Truncated first upload.",
        behavior: &[
            "The first PUT per path is cut off after `first_attempt_bytes` body bytes: the connection closes with no response and nothing is stored.",
            "Later PUT attempts on that path are read fully and stored.",
        ],
        invariant: "A cut attempt stores nothing, and a retried attempt completes the upload with the object stored whole.",
    },
    ScenarioDoc {
        id: "cut-body",
        name: "Truncated first download.",
        behavior: &[
            "The first success (200) GET per path writes honest status and headers, then only `after_bytes` body bytes and closes the connection.",
            "With `--fake-length` the `Content-Length` header lies 1024 bytes high, so the honest-looking answer still breaks mid-body.",
            "Later GETs serve the object whole, so a retry resuming from the part file completes the download.",
        ],
        invariant: "A mid-body break is a retryable transport failure: the retry resumes from the part file and the download lands byte-perfect, with or without the lying header.",
    },
    ScenarioDoc {
        id: "drop-connection",
        name: "Connection reset after the request head.",
        behavior: &[
            "The first request per path (any method) is reset right after its head (request line and headers) is read: the body is never read, nothing is answered and nothing is stored.",
            "Later requests are served normally.",
        ],
        invariant: "A reset before any answer is retryable like any broken transport.",
    },
    ScenarioDoc {
        id: "freeze-upload",
        name: "Held upload.",
        behavior: &[
            "PUT connections are held right after the head: the body is never read and no response is ever written, so the write side must detect the stall.",
            "The hold is bounded (about 15 minutes), then the connection is dropped.",
            "GET and HEAD behave like `atomic`.",
        ],
        invariant: "A stalled upload is aborted by the attempt watchdog instead of hanging forever.",
    },
    ScenarioDoc {
        id: "sizeless",
        name: "Success answers without Content-Length.",
        behavior: &[
            "Success (200) GET and HEAD answers for present objects carry no `Content-Length`: the proxy case.",
            "A client must refuse to treat such objects as absent (the digest comparison would be skipped).",
            "Every other answer (206, 416, 404) keeps `Content-Length`.",
            "PUTs behave like `atomic`.",
        ],
        invariant: "A 2xx answer without a length is a broken answer, never an absent object: the client refuses instead of overwriting.",
    },
    ScenarioDoc {
        id: "slow",
        name: "Slow body.",
        behavior: &[
            "GET and HEAD bodies are written in `chunk_size` pieces, sleeping `chunk_delay_ms` between pieces.",
            "PUT bodies are read normally.",
        ],
        invariant: "Stall detection aborts the slow attempt, the retry loop continues, and an honest refusal is the last resort.",
    },
    ScenarioDoc {
        id: "foreign-marker",
        name: "Foreign digest in the marker.",
        behavior: &[
            "Stored `.sha256` markers get their 64-char digest replaced by 64 zeros (the digest of a foreign object).",
            "Everything else is stored verbatim.",
        ],
        invariant: "A remote object whose sibling belongs to a foreign object is never overwritten: the digest check refuses and the partial file is dropped.",
    },
    ScenarioDoc {
        id: "markerless",
        name: "Markers silently dropped.",
        behavior: &[
            "`.sha256` markers are acknowledged (201 Created) but never stored.",
            "Non-marker bytes are stored normally.",
        ],
        invariant: "Marker symmetry survives a markerless server: `up` re-uploads the markers and `down` writes the marker it computed from the received bytes.",
    },
    ScenarioDoc {
        id: "auth-401",
        name: "Basic auth challenge (401).",
        behavior: &[
            "Every request requires `Authorization: Basic base64(user:pass)`.",
            "Otherwise the answer is `401` with `WWW-Authenticate: Basic realm=\"nexus\"`.",
            "Valid credentials behave like `atomic`.",
        ],
        invariant: "Auth failures fail fast with no retries, and the failure names the URL.",
    },
    ScenarioDoc {
        id: "auth-403",
        name: "Basic auth refusal (403).",
        behavior: &[
            "Every request requires `Authorization: Basic base64(user:pass)`.",
            "Anything else answers `403 Forbidden` with a `forbidden` body, on any method (unlike `auth-401`, which answers `401`).",
            "Valid credentials behave like `atomic`.",
        ],
        invariant: "A 403 refusal maps onto the auth error class exactly like a 401 challenge.",
    },
    ScenarioDoc {
        id: "doc-drift",
        name: "Version document drift.",
        behavior: &[
            "Behaves like `atomic` until drift is enabled through the library API (`MockNexus::enable_drift`).",
            "Afterwards every GET of a `*/version.json` path serves a synthesized version document with a ghost artifact.",
            "PUTs keep storing verbatim, and `MockNexus::disable_drift` restores store-backed responses.",
        ],
        invariant: "A version document is an ordinary name with no protocol meaning: the client serves what the server serves instead of special-casing it.",
    },
    ScenarioDoc {
        id: "flaky",
        name: "503 burst.",
        behavior: &[
            "The first `first_failures` requests per path (any method) get `503 Service Unavailable`.",
            "Later requests are served normally.",
        ],
        invariant: "One invocation recovers a 503 burst through retries.",
    },
    ScenarioDoc {
        id: "rate-limit",
        name: "429 with Retry-After.",
        behavior: &[
            "The first `first_429s` requests per path (any method) answer `429 Too Many Requests` with a `Retry-After: <seconds>` header and an empty body.",
            "Later requests are served like `atomic`.",
        ],
        invariant: "The client honors the `Retry-After` pause, spends attempts on the 429s and still lands the transfer in one invocation.",
    },
    ScenarioDoc {
        id: "redirect",
        name: "Redirect on reads.",
        behavior: &[
            "GET and HEAD requests answer `301 Moved Permanently` with `Location: <location_path>` (same host, the path from the repository root) and an empty body.",
            "Every other method behaves like `atomic`.",
        ],
        invariant: "Redirects are never followed: a 301 on a read surfaces as a plain transport error.",
    },
    ScenarioDoc {
        id: "readonly",
        name: "Read-only repository.",
        behavior: &[
            "Every DELETE is refused with `403 Forbidden`: the read-only repository.",
            "Nothing is ever removed from the store.",
            "GET, HEAD and PUT behave like `atomic`.",
        ],
        invariant: "A refused deletion changes nothing: the client reports the refusal and a rerun stays safe.",
    },
    ScenarioDoc {
        id: "no-service",
        name: "No service REST endpoint.",
        behavior: &[
            "The service REST endpoint (`/service/rest/v1/repositories`) answers `404` like a store miss: an installation without the management API (an old Nexus, or a non-Sonatype server).",
            "Storage behavior is `atomic`; only `service repos` notices.",
        ],
        invariant: "The service REST endpoint is optional: the client refuses with a hint that names the server root, and every storage command is unaffected.",
    },
];

/// Render `docs/reference/invariants.md` from [`SCENARIO_DOCS`].
/// The drift test fails when the file and this rendering diverge.
#[must_use]
pub fn render_invariants_markdown() -> String {
    let mut out = String::from(INTRO);
    for doc in SCENARIO_DOCS {
        out.push_str("\n## `");
        out.push_str(doc.id);
        out.push_str("`\n\n");
        out.push_str(doc.name);
        out.push('\n');
        out.push('\n');
        for sentence in doc.behavior {
            out.push_str(sentence);
            out.push('\n');
        }
        out.push('\n');
        out.push_str("**Pinned invariant:** ");
        out.push_str(doc.invariant);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{render_invariants_markdown, SCENARIO_DOCS};
    use crate::scenario::SCENARIOS;

    /// The checked-in page, addressed from this crate's manifest directory.
    fn page_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/invariants.md")
    }

    #[test]
    fn invariant_rows_match_the_scenario_table() {
        let ids: Vec<&str> = SCENARIO_DOCS.iter().map(|doc| doc.id).collect();
        assert_eq!(
            ids, SCENARIOS,
            "SCENARIO_DOCS must carry one row per SCENARIOS entry, in order"
        );
    }

    #[test]
    fn invariants_page_drift() {
        let page = page_path();
        let rendered = render_invariants_markdown();
        if std::env::var_os("UPDATE_INVARIANTS").is_some() {
            std::fs::write(&page, rendered).expect("write docs/reference/invariants.md");
            return;
        }
        let on_disk = std::fs::read_to_string(&page)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", page.display()));
        assert_eq!(
            on_disk, rendered,
            "docs/reference/invariants.md drifted from the scenario table; regenerate with `UPDATE_INVARIANTS=1 cargo test -p mock-nexus invariant`"
        );
    }
}
