//! The service REST surface: the repositories document, the generated asset search pages, the audit-litter facts and the EULA disclaimer.
//! The dispatch over the `/service/rest/...` paths (including the inline status answers) lives in `http`.
//! These helpers build the bodies those endpoints answer with.

use super::support::{lock, percent_decode, plain, query_param};
use crate::scenario::Scenario;
use crate::server::{Request, Resp};
use crate::store::Shared;

pub(super) const EULA_DISCLAIMER: &str = "You are accepting an agreement. By using this Nexus repository manager you are agreeing to the Sonatype End User License Agreement (EULA) as outlined at https://links.sonatype.com/products/nxrm3/docs/ce-onboarding";

/// How many assets the assets-pagination scenario serves for `raw-main`.
const ASSETS_TOTAL: usize = 35;

/// How many assets one search page carries under `continuationToken` pagination.
const ASSETS_PAGE: usize = 10;

/// The page the pinned Nexus really serves: 50 rows, whatever `pageSize` says.
/// Measured on 3.96.3 (`stand/real-nexus/search-pages.sh`), so the page bound arithmetic
/// of the `assets-prefix-q` fixture is the production arithmetic, not a mock convenience.
const SEARCH_ROWS_PER_PAGE: usize = 50;

/// The batch directory of the `assets-prefix-q` scenario: the directory a scoped listing asks for.
const PREFIX_Q_DIR: usize = 300;

/// The name-continuing sibling of the batch directory: only a `q` without the trailing slash
/// drags it in, the `flat` against `flat2` case the measurement pins.
const PREFIX_Q_SIBLING: usize = 4;

/// A copy of the batch directory under another parent: the loose `q` match (a path substring)
/// drags it into every scoped page, so only the client-side prefix filter keeps it out.
const PREFIX_Q_DECOY: usize = 200;

/// The bulk that takes the repository past 5000 rows: 5539 rows are 111 pages at 50 a page,
/// past the 100-page bound a listing used to carry.
const PREFIX_Q_BULK: usize = 5000;

/// The generated search corpus of a scenario, in the order the server pages it.
/// The paths carry the leading slash, the shape a real Nexus reports.
fn corpus(scenario: &Scenario) -> Vec<String> {
    let mut paths = Vec::new();
    for i in 0..ASSETS_TOTAL {
        paths.push(format!("/v0.9.0-assets/asset-{i:03}.bin"));
    }
    if *scenario == Scenario::AssetsPrefixQ {
        for i in 0..PREFIX_Q_DIR {
            paths.push(format!("/v1.1.0-dir/dir-{i:04}.bin"));
        }
        for i in 0..PREFIX_Q_SIBLING {
            paths.push(format!("/v1.1.0-dir2/dir2-{i:02}.bin"));
        }
        for i in 0..PREFIX_Q_BULK {
            paths.push(format!("/v1.0.0-bulk/bulk-{i:04}.bin"));
        }
        for i in 0..PREFIX_Q_DECOY {
            paths.push(format!("/backup/v1.1.0-dir/decoy-{i:04}.bin"));
        }
    }
    paths
}

/// The search `q` match of the pinned server: a case-sensitive substring over the path, taken
/// after the client's percent-encoding is decoded.
/// It is loose on purpose, exactly as measured: `ench/flat/` matches `/bench/flat/`, and a value
/// without the trailing slash (`bench/flat`) drags the name-continuing `bench/flat2/` in.
/// A superset is safe for a listing, which filters client-side.
/// A narrower set would not be.
pub(super) fn q_matches(q: &str, path: &str) -> bool {
    path.contains(q)
}

/// One search row, the shape `/search/assets` reports.
fn search_item(base: &str, path: &str, seed: usize) -> String {
    let sha = format!("{seed:064x}");
    // The size is synthetic like the digest: the generated corpus has no bytes
    // behind it. The field name is the real one (`fileSize`, measured against
    // the pinned 3.96.3), so a parser that reads the wrong key fails here too.
    let size = 1000 + (seed % 500);
    format!(
        "{{\"downloadUrl\":\"{base}/repository/raw-main{path}\",\"path\":\"{path}\",\"id\":\"raw-main:{seed:06}\",\"repository\":\"raw-main\",\"format\":\"raw\",\"fileSize\":{size},\"checksum\":{{\"sha256\":\"{sha}\"}},\"lastModified\":\"2026-10-09T12:00:00.000+00:00\"}}"
    )
}

/// Serve one page of the generated asset list: items for the current token, plus the next token until the list is exhausted.
/// `q`, when present, matches paths as a loose substring after decoding, the way the real server does.
pub(super) fn assets_page(shared: &Shared, req: &Request) -> Resp {
    let q = query_param(&req.target, "q").map(percent_decode);
    let token = query_param(&req.target, "continuationToken");
    let start = token.and_then(|t| t.parse::<usize>().ok()).unwrap_or(0);
    let page_size = if shared.scenario == Scenario::AssetsPrefixQ {
        SEARCH_ROWS_PER_PAGE
    } else {
        ASSETS_PAGE
    };
    let all = corpus(&shared.scenario);
    let matching: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, path)| q.as_deref().is_none_or(|q| q_matches(q, path)))
        .map(|(i, _)| i)
        .collect();
    let total = matching.len();
    let base = base_of(shared);
    let page: Vec<String> = matching
        .into_iter()
        .skip(start)
        .take(page_size)
        .map(|i| search_item(&base, &all[i], i))
        .collect();
    let body = if start + page.len() < total {
        format!(
            "{{\"items\": [{}], \"continuationToken\": \"{}\"}}",
            page.join(", "),
            start + page_size
        )
    } else {
        format!("{{\"items\": [{}]}}", page.join(", "))
    };
    plain(200, body.as_bytes(), None)
}

/// The search answer for the audit-litter scenario: every seeded object listed
/// with its search fact, clean pair included, markers included.
/// The facts are the cross-check surface: the clean marker's digest agrees with
/// its fact, and the report stays honest about what the index proves.
pub(super) fn audit_litter_page(shared: &Shared) -> Resp {
    const CLEAN_DIGEST: &str = "a22ae4f83248fb43415404da410560378910398baf65f234d8de8cde3538351d";
    let base = base_of(shared);
    let item = |path: &str, digest: &str| {
        format!(
            "{{\"downloadUrl\":\"{base}/repository/raw-main/1.0.0/{path}\",\"path\":\"/1.0.0/{path}\",\"id\":\"raw-main:{path}\",\"repository\":\"raw-main\",\"format\":\"raw\",\"checksum\":{{\"sha256\":\"{digest}\"}},\"lastModified\":\"2026-10-13T12:00:00.000+00:00\"}}"
        )
    };
    let items = [
        item(
            "broken/app.bin",
            "cc03e1c1d5f1d2d3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7",
        ),
        item(
            "broken/app.bin.sha256",
            "bb02e1c1d5f1d2d3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7",
        ),
        item("clean/app.bin", CLEAN_DIGEST),
        item("clean/app.bin.sha256", CLEAN_DIGEST),
        item(
            "dangling/app.bin.sha256",
            "aa02e1c1d5f1d2d3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7",
        ),
        item(
            "orphan/app.bin",
            "cc02e1c1d5f1d2d3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7",
        ),
    ];
    let body = format!("{{\"items\": [{}]}}", items.join(", "));
    plain(200, body.as_bytes(), None)
}

/// The origin the instance was started on, without the trailing slash.
pub(super) fn base_of(shared: &Shared) -> String {
    lock(&shared.store)
        .get("service/rest/v1/repositories")
        .and_then(|doc| {
            String::from_utf8_lossy(doc)
                .split("\"url\":\"")
                .nth(1)
                .map(str::to_owned)
        })
        .and_then(|rest| rest.split("/repository/").next().map(str::to_owned))
        .unwrap_or_default()
}

/// The repositories document the seed wrote: verbatim from the store, so trimmed and full shapes both ride it.
pub(super) fn seed_repositories_doc(shared: &Shared) -> String {
    lock(&shared.store)
        .get("service/rest/v1/repositories")
        .map(|doc| String::from_utf8_lossy(doc).into_owned())
        .unwrap_or_else(|| "[]".to_owned())
}
