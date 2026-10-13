//! The composable behavior description: [`Spec`] drives the mock's knobs.
//!
//! A [`Scenario`] is a named preset; a [`Spec`] is the same knobs laid out as
//! independent layers a caller can compose and tweak.
//! `Scenario -> Spec` is lossless; the drift guards stay on the enum, the
//! runtime reads the spec.

use crate::scenario::Scenario;

/// The composable behavior of one hosted instance.
#[derive(Debug, Clone, Default)]
pub struct Spec {
    /// Basic-auth gate: the expected `Authorization` token, base64 of `user:pass`.
    pub auth_b64: Option<String>,
    /// Cut the first PUT per path after this many body bytes.
    pub partial_first_put: Option<usize>,
    /// Cut the first success GET per path: (body bytes, lie about Content-Length).
    pub cut_first_get: Option<(usize, bool)>,
    /// Answer GET/HEAD bodies in delayed chunks.
    pub drip: Option<super::server::Drip>,
    /// Success GET/HEAD answers hide `Content-Length`.
    pub sizeless: bool,
    /// The first N requests per path answer 429 with a Retry-After of this many seconds.
    pub rate_first: Option<(usize, u64)>,
    /// The first N requests per path answer 503.
    pub flaky_first: Option<u32>,
    /// Refuse every ranged GET with the whole body (200).
    pub ignore_range: bool,
    /// Drop markers instead of storing them.
    pub markerless: bool,
    /// Replace marker digests with zeros.
    pub foreign_marker: bool,
    /// Refuse every DELETE.
    pub read_only: bool,
    /// Redirect reads to this path with an empty 301.
    pub redirect: Option<String>,
    /// Reset the first request per path.
    pub drop_connection: bool,
    /// Hold PUT connections without reading or answering.
    pub freeze_upload: bool,
    /// Serve a drifted version document once drift is enabled.
    pub doc_drift: bool,
}

impl Spec {
    /// The spec of a plain correct server.
    pub fn atomic() -> Self {
        Self::default()
    }

    /// A Basic-auth gate from plain credentials.
    pub fn with_auth(mut self, user: &str, pass: &str) -> Self {
        self.auth_b64 = Some(crate::base64::encode(format!("{user}:{pass}").as_bytes()));
        self
    }

    /// Cut the first PUT per path after `bytes` body bytes.
    pub fn with_partial_put(mut self, bytes: usize) -> Self {
        self.partial_first_put = Some(bytes);
        self
    }

    /// Cut the first success GET per path after `bytes` body bytes, optionally lying about `Content-Length`.
    pub fn with_cut_body(mut self, bytes: usize, fake_length: bool) -> Self {
        self.cut_first_get = Some((bytes, fake_length));
        self
    }

    /// Serve GET/HEAD bodies in delayed chunks.
    pub fn with_drip(mut self, delay_ms: u64, chunk_size: usize) -> Self {
        self.drip = Some(super::server::Drip {
            delay_ms,
            chunk_size,
        });
        self
    }

    /// Hide `Content-Length` on success GET/HEAD answers.
    pub fn with_sizeless(mut self) -> Self {
        self.sizeless = true;
        self
    }

    /// Answer the first `n` requests per path with 429 and this Retry-After.
    pub fn with_rate_limit(mut self, n: usize, retry_after_secs: u64) -> Self {
        self.rate_first = Some((n, retry_after_secs));
        self
    }

    /// Answer the first `n` requests per path with 503.
    pub fn with_flaky(mut self, n: u32) -> Self {
        self.flaky_first = Some(n);
        self
    }

    /// Answer every ranged GET with the whole body.
    pub fn with_ignore_range(mut self) -> Self {
        self.ignore_range = true;
        self
    }

    /// Drop `.sha256` PUTs instead of storing them.
    pub fn with_markerless(mut self) -> Self {
        self.markerless = true;
        self
    }

    /// Zero out every stored marker's digest.
    pub fn with_foreign_marker(mut self) -> Self {
        self.foreign_marker = true;
        self
    }

    /// Refuse every DELETE with 403.
    pub fn with_read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Redirect every read to this path with an empty 301.
    pub fn with_redirect(mut self, location_path: &str) -> Self {
        self.redirect = Some(location_path.to_owned());
        self
    }

    /// Reset the first request per path right after its head.
    pub fn with_drop_connection(mut self) -> Self {
        self.drop_connection = true;
        self
    }

    /// Hold PUT connections without reading or answering.
    pub fn with_freeze_upload(mut self) -> Self {
        self.freeze_upload = true;
        self
    }

    /// Serve a drifted version document once drift is enabled.
    pub fn with_doc_drift(mut self) -> Self {
        self.doc_drift = true;
        self
    }
}

impl From<&Scenario> for Spec {
    fn from(scenario: &Scenario) -> Self {
        let mut spec = Self::default();
        match scenario {
            Scenario::PartialPut {
                first_attempt_bytes,
            } => spec.partial_first_put = Some(*first_attempt_bytes),
            Scenario::CutBody {
                after_bytes,
                fake_length,
            } => spec.cut_first_get = Some((*after_bytes, *fake_length)),
            Scenario::DropConnection => spec.drop_connection = true,
            Scenario::FreezeUpload => spec.freeze_upload = true,
            Scenario::Sizeless => spec.sizeless = true,
            Scenario::Slow {
                chunk_delay_ms,
                chunk_size,
            } => {
                spec.drip = Some(super::server::Drip {
                    delay_ms: *chunk_delay_ms,
                    chunk_size: *chunk_size,
                })
            }
            Scenario::ForeignMarker => spec.foreign_marker = true,
            Scenario::Markerless => spec.markerless = true,
            Scenario::Auth401 { user, pass } => {
                spec.auth_b64 = Some(crate::base64::encode(format!("{user}:{pass}").as_bytes()))
            }
            Scenario::Auth403 { user, pass } => {
                spec.auth_b64 = Some(crate::base64::encode(format!("{user}:{pass}").as_bytes()))
            }
            Scenario::DocDrift => spec.doc_drift = true,
            Scenario::Flaky { first_failures } => spec.flaky_first = Some(*first_failures),
            Scenario::RateLimit {
                first_429s,
                retry_after_secs,
            } => spec.rate_first = Some((*first_429s, *retry_after_secs)),
            Scenario::Redirect { location_path } => spec.redirect = Some(location_path.clone()),
            Scenario::ReadOnly => spec.read_only = true,
            Scenario::NoService => {}
            Scenario::Search400 => {}
            Scenario::ServiceStatusEmpty => {}
            Scenario::ServiceStatusVersion => {}
            Scenario::ServiceStatusDown => {}
            Scenario::RepoCollectionTrimmed => {}
            Scenario::RepoCollectionScoped { .. } => {}
            Scenario::RepoDetailAdmin { .. } => {}
            Scenario::RepoPrefixMatch => {}
            Scenario::AssetsPagination => {}
            Scenario::AssetsAbsent => {}
            Scenario::AssetsPrefixQ => {}
            Scenario::EulaGate => {}
            Scenario::EulaAbsent => {}
            Scenario::DetailInHint => {}
            Scenario::IgnoreRange => spec.ignore_range = true,
            Scenario::AuditLitter => {}
            Scenario::Atomic => {}
        }
        spec
    }
}
