//! Storage-side helpers: range parsing, drift synthesis, marker transforms.

use std::sync::atomic::Ordering::Relaxed;

use super::support::lock;
use crate::store::Shared;

/// Parse a `Range` header for resumable and segmented GETs.
/// Recognized: the single open form `bytes=N-` (from byte `N` to the end) and the closed form `bytes=N-M` (bytes `N` through `M` inclusive).
/// Anything else, whether another unit, multiple ranges, the suffix `-N` form or malformed values, yields `None` and the response is served in full.
pub(super) fn range_span(header: Option<&str>) -> Option<(usize, Option<usize>)> {
    let value = header?;
    let (unit, spec) = value.split_once('=')?;
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return None;
    }
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let end = end.trim();
    if end.is_empty() {
        return Some((start, None));
    }
    Some((start, Some(end.parse::<usize>().ok()?)))
}

/// True when the path designates a version document (`<version>/version.json`).
pub(super) fn is_version_path(path: &str) -> bool {
    path.ends_with("/version.json")
}

/// First `/`-separated segment of a store path (the version directory).
pub(super) fn first_segment(path: &str) -> &str {
    path.split('/').next().unwrap_or_default()
}

/// Synthesized drifting version document, same shape as protocol §4.1 but listing a ghost artifact instead of the real ones.
pub(super) fn drift_doc(version: &str) -> Vec<u8> {
    format!("{{\"schema_version\":1,\"version\":\"{version}\",\"artifacts\":[\"ghost.zip\"]}}")
        .into_bytes()
}

/// Replace the 64-char digest of the marker's first line with 64 zeros (simulating a sibling written for a foreign object).
/// Everything else is stored verbatim.
/// Shorter payloads pass through unchanged.
pub(super) fn zero_digest(marker: &[u8]) -> Vec<u8> {
    let line_end = marker
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(marker.len());
    let mut out = marker.to_vec();
    if line_end >= 64 {
        out[..64].fill(b'0');
    }
    out
}

/// Apply marker mutations for `.sha256` PUTs.
/// `None` means "acknowledge with 201 but store nothing" (markerless).
pub(super) fn transform_put(shared: &Shared, path: &str, body: &[u8]) -> Option<Vec<u8>> {
    if !path.ends_with(".sha256") {
        return Some(body.to_vec());
    }
    if shared.spec.markerless {
        return None;
    }
    if shared.spec.foreign_marker {
        return Some(zero_digest(body));
    }
    Some(body.to_vec())
}

/// GET payload: the synthesized drifting version document once drift is enabled, otherwise whatever is in the store.
pub(super) fn drift_or_store(shared: &Shared, path: &str) -> Option<Vec<u8>> {
    if shared.drift.load(Relaxed) && is_version_path(path) {
        return Some(drift_doc(first_segment(path)));
    }
    lock(&shared.store).get(path).cloned()
}
