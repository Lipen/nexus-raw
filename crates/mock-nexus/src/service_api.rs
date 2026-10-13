//! The service REST surface: the repositories document, the generated asset search pages, the audit-litter facts and the EULA disclaimer.
//! The dispatch over the `/service/rest/...` paths (including the inline status answers) lives in `http`.
//! These helpers build the bodies those endpoints answer with.

use super::support::{lock, plain, query_param};
use crate::server::{Request, Resp};
use crate::store::Shared;

pub(super) const EULA_DISCLAIMER: &str = "You are accepting an agreement. By using this Nexus repository manager you are agreeing to the Sonatype End User License Agreement (EULA) as outlined at https://links.sonatype.com/products/nxrm3/docs/ce-onboarding";

/// How many assets the assets-pagination scenario serves for `raw-main`.
const ASSETS_TOTAL: usize = 35;

/// How many assets one search page carries under `continuationToken` pagination.
const ASSETS_PAGE: usize = 10;

/// Serve one page of the generated asset list: items for the current token, plus the next token until the list is exhausted.
/// `q`, when present, filters asset paths by substring, the way the real server does.
pub(super) fn assets_page(shared: &Shared, req: &Request) -> Resp {
    let q = query_param(&req.target, "q");
    let token = query_param(&req.target, "continuationToken");
    let start = token.and_then(|t| t.parse::<usize>().ok()).unwrap_or(0);
    let mut all = Vec::new();
    for i in 0..ASSETS_TOTAL {
        let path = format!("/v0.9.0-assets/asset-{i:03}.bin");
        if let Some(q) = q {
            if !path.contains(q) {
                continue;
            }
        }
        let sha = format!("{i:064x}");
        let base = base_of(shared);
        all.push(format!(
            "{{\"downloadUrl\":\"{base}repository/raw-main/v0.9.0-assets/asset-{i:03}.bin\",\"path\":\"{path}\",\"id\":\"raw-main:{i:03}\",\"repository\":\"raw-main\",\"format\":\"raw\",\"checksum\":{{\"sha256\":\"{sha}\"}},\"lastModified\":\"2026-10-09T12:00:00.000+00:00\"}}"
        ));
    }
    let total = all.len();
    let page: Vec<String> = all.into_iter().skip(start).take(ASSETS_PAGE).collect();
    let body = if start + page.len() < total {
        format!(
            "{{\"items\": [{}], \"continuationToken\": \"{}\"}}",
            page.join(", "),
            start + ASSETS_PAGE
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
            "{{\"downloadUrl\":\"{base}repository/raw-main/1.0.0/{path}\",\"path\":\"/1.0.0/{path}\",\"id\":\"raw-main:{path}\",\"repository\":\"raw-main\",\"format\":\"raw\",\"checksum\":{{\"sha256\":\"{digest}\"}},\"lastModified\":\"2026-10-13T12:00:00.000+00:00\"}}"
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
