//! The scenario contract: the enum and the canonical name table.
//! The behavior itself lives in `http` (the request pipeline), `service_api` (the REST pages) and `storage` (the store-side transforms).
//! This module is what the drift guards pin.

/// Scenario names understood by the `mock-nexus` binary and by test harnesses.
impl Scenario {
    /// The canonical scenario name, matching the [`SCENARIOS`] table entry.
    pub fn as_str(&self) -> &'static str {
        // The table is the single source: the position of the variant IS the name.
        match self {
            Scenario::Atomic => "atomic",
            Scenario::PartialPut { .. } => "partial-put",
            Scenario::CutBody { .. } => "cut-body",
            Scenario::DropConnection => "drop-connection",
            Scenario::FreezeUpload => "freeze-upload",
            Scenario::Sizeless => "sizeless",
            Scenario::Slow { .. } => "slow",
            Scenario::ForeignMarker => "foreign-marker",
            Scenario::Markerless => "markerless",
            Scenario::Auth401 { .. } => "auth-401",
            Scenario::Auth403 { .. } => "auth-403",
            Scenario::DocDrift => "doc-drift",
            Scenario::Flaky { .. } => "flaky",
            Scenario::RetryAfter503 { .. } => "retry-after-503",
            Scenario::RateLimit { .. } => "rate-limit",
            Scenario::Redirect { .. } => "redirect",
            Scenario::ReadOnly => "readonly",
            Scenario::NoService => "no-service",
            Scenario::Search400 => "search-400",
            Scenario::ServiceStatusEmpty => "service-status-empty",
            Scenario::ServiceStatusVersion => "service-status-version",
            Scenario::ServiceStatusDown => "service-status-down",
            Scenario::RepoCollectionTrimmed => "repo-collection-trimmed",
            Scenario::RepoCollectionScoped { .. } => "repo-collection-scoped",
            Scenario::RepoDetailAdmin { .. } => "repo-detail-admin",
            Scenario::RepoPrefixMatch => "repo-prefix-match",
            Scenario::AssetsPagination => "assets-pagination",
            Scenario::AssetsAbsent => "assets-absent",
            Scenario::AssetsPrefixQ => "assets-prefix-q",
            Scenario::EulaGate => "eula-gate",
            Scenario::EulaAbsent => "eula-absent",
            Scenario::SearchLag { .. } => "search-lag",
            Scenario::DetailInHint => "detail-in-hint",
            Scenario::IgnoreRange => "ignore-range",
            Scenario::AuditLitter => "audit-litter",
        }
    }
}

pub const SCENARIOS: &[&str] = &[
    "atomic",
    "partial-put",
    "cut-body",
    "drop-connection",
    "freeze-upload",
    "sizeless",
    "slow",
    "foreign-marker",
    "markerless",
    "auth-401",
    "auth-403",
    "doc-drift",
    "flaky",
    "retry-after-503",
    "rate-limit",
    "redirect",
    "readonly",
    "no-service",
    "search-400",
    "service-status-empty",
    "service-status-version",
    "service-status-down",
    "repo-collection-trimmed",
    "repo-collection-scoped",
    "repo-detail-admin",
    "repo-prefix-match",
    "assets-pagination",
    "assets-absent",
    "assets-prefix-q",
    "eula-gate",
    "eula-absent",
    "search-lag",
    "detail-in-hint",
    "ignore-range",
    // wave-150 tail
    "audit-litter",
];

/// Failure scenario a [`MockNexus`](crate::MockNexus) server simulates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scenario {
    /// Straight-through storage: every fully-read request is served normally.
    Atomic,
    /// The first PUT per path is cut off after `first_attempt_bytes` body bytes: the connection closes with no response and nothing is stored.
    /// Later PUT attempts on that path are read fully and stored.
    PartialPut { first_attempt_bytes: usize },
    /// The first success (200) GET per path writes honest status and headers, then only `after_bytes` body bytes and closes the connection.
    /// With `fake_length` the `Content-Length` header lies `FAKE_LENGTH_LIE` bytes high, so the honest-looking answer still breaks mid-body.
    /// Later GETs serve the object whole, so a retry resuming from the part file completes the download.
    CutBody {
        after_bytes: usize,
        fake_length: bool,
    },
    /// The first request per path (any method) is reset right after its head (request line and headers) is read: the body is never read, nothing is answered and nothing is stored.
    /// Later requests are served normally.
    DropConnection,
    /// PUT connections are held right after the head: the body is never read and no response is ever written, so the write side must detect the stall.
    /// The hold is bounded (about 15 minutes), then the connection is dropped.
    /// GET/HEAD behave like [`Scenario::Atomic`].
    FreezeUpload,
    /// Success (200) GET and HEAD answers for present objects carry no `Content-Length`: the proxy case.
    /// A client must refuse to treat such objects as absent (the digest comparison would be skipped).
    /// Every other answer (206, 416, 404) keeps `Content-Length`.
    /// PUTs behave like [`Scenario::Atomic`].
    Sizeless,
    /// GET/HEAD bodies are written in `chunk_size` pieces, sleeping `chunk_delay_ms` between pieces.
    /// PUT bodies are read normally.
    Slow {
        chunk_delay_ms: u64,
        chunk_size: usize,
    },
    /// Stored `.sha256` markers get their 64-char digest replaced by 64 zeros (digest of a foreign object).
    /// Everything else is stored verbatim.
    ForeignMarker,
    /// `.sha256` markers are acknowledged (201 Created) but never stored.
    /// Non-marker bytes are stored normally.
    Markerless,
    /// Every request requires `Authorization: Basic base64(user:pass)`.
    /// Otherwise 401 with `WWW-Authenticate: Basic realm="nexus"`.
    /// Valid credentials behave like [`Scenario::Atomic`].
    Auth401 { user: String, pass: String },
    /// Every request requires `Authorization: Basic base64(user:pass)`.
    /// Anything else answers `403 Forbidden` with a `forbidden` body, on any method (unlike [`Scenario::Auth401`], which answers `401`).
    /// Valid credentials behave like [`Scenario::Atomic`].
    Auth403 { user: String, pass: String },
    /// Behaves like [`Scenario::Atomic`] until [`MockNexus::enable_drift`](crate::MockNexus::enable_drift).
    /// Afterwards every GET of a `*/version.json` path serves a synthesized version document with a ghost artifact.
    /// PUTs keep storing verbatim, and [`MockNexus::disable_drift`](crate::MockNexus::disable_drift) restores store-backed responses.
    DocDrift,
    /// The first `first_failures` requests per path (any method) get 503 Service Unavailable.
    /// Later requests are served normally.
    Flaky { first_failures: u32 },
    /// The first request per path (any method) gets 503 Service Unavailable with a `Retry-After` pause of `retry_after_secs`.
    /// Later requests are served normally.
    RetryAfter503 { retry_after_secs: u64 },
    /// The first `first_429s` requests per path (any method) answer `429 Too Many Requests` with a `Retry-After: <retry_after_secs>` header and an empty body.
    /// Later requests are served like [`Scenario::Atomic`].
    RateLimit {
        first_429s: usize,
        retry_after_secs: u64,
    },
    /// GET and HEAD requests answer `301 Moved Permanently` with `Location: <location_path>` (same host, the path from the repository root) and an empty body.
    /// Every other method behaves like [`Scenario::Atomic`].
    Redirect { location_path: String },
    /// The store carries one clean pair (`clean/app.bin` + an honest marker), an orphan (`orphan/app.bin` without a marker), a dangling marker (`dangling/app.bin.sha256` without bytes) and an unparseable marker (`broken/app.bin.sha256` with a body outside the `sha256sum -c` format).
    /// GET/HEAD/PUT behave like [`Scenario::Atomic`]: the litter is seeded data, not wire behavior.
    /// The `nxr audit` structural mode classifies all three classes and never refuses; `--bytes` still verifies the clean pair.
    AuditLitter,
    /// Every DELETE is refused with `403 Forbidden`: the read-only repository.
    /// Nothing is ever removed from the store.
    /// GET/HEAD/PUT behave like [`Scenario::Atomic`].
    ReadOnly,
    /// The service REST endpoint (`/service/rest/v1/repositories`) answers `404` like a store miss:
    /// an installation without the management API (an old Nexus, or a non-Sonatype server).
    /// Storage behavior is [`Scenario::Atomic`].
    /// Only `service repos` notices.
    NoService,
    /// The search API (`/service/rest/v1/search/assets`) answers `400 Bad Request` when the `repository` query parameter names a repository the instance does not serve:
    /// a real Nexus refuses a repository-scoped search for an unknown repository with 400, before any storage is touched.
    /// Searches for the served repositories (`raw-main`, `raw-all`, the names of the seeded service document) and every other request behave like [`Scenario::Atomic`].
    Search400,
    /// `/service/rest/v1/status` and `/status/writable` answer `200` with an empty body: the Nexus 3.79 shape.
    /// Storage behavior is [`Scenario::Atomic`].
    ServiceStatusEmpty,
    /// `/service/rest/v1/status` answers `200` with a JSON body carrying a `version` field: the newer-server shape.
    /// `/status/writable` answers `200` empty.
    /// Storage behavior is [`Scenario::Atomic`].
    ServiceStatusVersion,
    /// `/service/rest/v1/status` answers `503` with a `down` body: the server probes as alive but unhealthy.
    /// Storage behavior is [`Scenario::Atomic`].
    ServiceStatusDown,
    /// The repositories document seeds `size` numbers and non-empty `attributes` for every entry: no shape change.
    /// Included so client behavior against a fuller collection is pinned by name.
    /// Storage behavior is [`Scenario::Atomic`].
    RepoCollectionTrimmed,
    /// The repositories document differs by caller: an anonymous request sees one entry, a request with the scenario credentials sees two.
    /// Storage behavior is [`Scenario::Atomic`].
    RepoCollectionScoped { user: String, pass: String },
    /// `GET /service/rest/v1/repositories/{format}/{type}/{name}` answers the full settings JSON for valid credentials and `403` with a `nx-admin required` body otherwise.
    /// Storage behavior is [`Scenario::Atomic`].
    RepoDetailAdmin { user: String, pass: String },
    /// A plain repositories collection for the client-side prefix-resolution tests.
    /// Behavior is identical to [`Scenario::Atomic`]: the scenario pins the conformance name.
    RepoPrefixMatch,
    /// The search API serves 35 generated assets for `raw-main` across pages of 10 via `continuationToken`, honoring `q` as a substring filter.
    /// Storage behavior is [`Scenario::Atomic`].
    AssetsPagination,
    /// The search API is absent (404): the honest degradation path for `service assets`.
    /// Storage behavior is [`Scenario::Atomic`].
    AssetsAbsent,
    /// The search API serves a generated repository for `raw-main`: the 35 assets of
    /// [`Scenario::AssetsPagination`], a 300-name directory, its name-continuing sibling,
    /// a 200-name copy of that directory under another parent and a 5000-name bulk,
    /// 50 rows a page, honoring `q` as the pinned server's loose path substring.
    /// 5539 rows are 111 pages: a repository-wide walk is past the 100-page bound a listing
    /// used to carry, while a scoped listing of the 300-name directory is 10.
    /// Storage behavior is [`Scenario::Atomic`].
    AssetsPrefixQ,
    /// The CE 3.79 EULA gate: `GET /v1/system/eula` presents the disclaimer, PUT answers `403` with the EULA body until
    /// `POST /v1/system/eula` echoes it back with `accepted: true` (204), after which PUTs store normally.
    /// Storage behavior is otherwise [`Scenario::Atomic`].
    EulaGate,
    /// `/v1/system/eula` answers 404: no gate on this server (pre-3.79 CE, PRO).
    /// Storage behavior is [`Scenario::Atomic`].
    EulaAbsent,
    /// The search index lags behind writes: for `lag_ms` after the first PUT of a run,
    /// `/service/rest/v1/search/assets` serves the empty page; afterwards it serves the
    /// store-backed answer like [`Scenario::Atomic`]. The first PUT starts the clock.
    /// Storage behavior is [`Scenario::Atomic`].
    SearchLag { lag_ms: u64 },
    /// Writes answer `403` with a short `please ask the administrator` body, so the client hint carries the server detail.
    /// Storage reads behave like [`Scenario::Atomic`].
    DetailInHint,
    /// Ranged GETs are answered like plain GETs: `200` with the whole body, the proxy that strips `Range`.
    /// Every other request behaves like [`Scenario::Atomic`].
    IgnoreRange,
}
