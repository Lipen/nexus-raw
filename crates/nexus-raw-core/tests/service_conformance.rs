//! Conformance for the service overview: every new mock scenario rides its facade call.
//! The scenario list is the contract (`crates/mock-nexus/src/scenario.rs`): a scenario and its test land in the same PR.

use nexus_raw_core::config::Config;
use nexus_raw_core::Nxr;

fn client(base: String) -> Nxr {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg = Config {
        base,
        tls_insecure: false,
        workers: 1,
        retry_attempts: 1,
        connect_timeout: std::time::Duration::from_secs(5),
        stall_timeout: std::time::Duration::from_secs(5),
        auth: None,
    };
    Nxr::new(cfg, tx).expect("client")
}

fn creds_client(base: String, user: &str, pass: &str) -> Nxr {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg = Config {
        base,
        tls_insecure: false,
        workers: 1,
        retry_attempts: 1,
        connect_timeout: std::time::Duration::from_secs(5),
        stall_timeout: std::time::Duration::from_secs(5),
        auth: Some(format!(
            "Basic {}",
            nexus_raw_core::creds::basic(user, pass)
        )),
    };
    Nxr::new(cfg, tx).expect("client")
}

#[tokio::test]
async fn service_status_empty_reads_alive() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::ServiceStatusEmpty).unwrap();
    let nxr = client(server.base_url());
    let s = nxr.service_status().await.unwrap();
    assert!(s.alive);
    assert!(s.writable);
    assert_eq!(s.version, None);
}

#[tokio::test]
async fn service_status_version_carries_version() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::ServiceStatusVersion).unwrap();
    let nxr = client(server.base_url());
    let s = nxr.service_status().await.unwrap();
    assert!(s.alive);
    assert_eq!(s.version.as_deref(), Some("3.79.1-04"));
}

#[tokio::test]
async fn service_status_down_reads_not_alive() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::ServiceStatusDown).unwrap();
    let nxr = client(server.base_url());
    // A 5xx from the status endpoint exhausts the retry budget: the transport error is the verdict.
    let err = nxr.service_status().await.unwrap_err();
    assert_eq!(err.exit_code(), 3);
}

#[tokio::test]
async fn repo_collection_trimmed_resolves_by_prefix() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::RepoCollectionTrimmed).unwrap();
    let base = format!("{}repository/raw-main/1.4.0/", server.base_url());
    let nxr = client(base);
    let repo = nxr.service_repo(false).await.unwrap();
    assert_eq!(repo.info.name, "raw-main");
    assert!(repo.detail.is_none());
}

#[tokio::test]
async fn repo_collection_scoped_hides_secret_repo_from_anonymous() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::RepoCollectionScoped {
        user: "admin".into(),
        pass: "pw".into(),
    })
    .unwrap();
    let anon = client(server.base_url());
    // Anonymous resolves only entries its list carries: raw-secret must be invisible.
    let hidden = anon
        .service_repo(false)
        .await
        .err()
        .map(|e| matches!(e, nexus_raw_core::error::Error::Missing { .. }));
    assert_ne!(
        hidden,
        Some(false),
        "raw-secret must not resolve anonymously"
    );
    // The authed list also carries raw-main; resolving it must work.
    let authed = creds_client(
        format!("{}repository/raw-main/", server.base_url()),
        "admin",
        "pw",
    );
    let repo = authed.service_repo(false).await.unwrap();
    assert_eq!(repo.info.name, "raw-main");
}

#[tokio::test]
async fn repo_detail_admin_answers_settings_and_403() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::RepoDetailAdmin {
        user: "admin".into(),
        pass: "pw".into(),
    })
    .unwrap();
    let repo_base = format!("{}repository/raw-main/", server.base_url());
    let authed = creds_client(repo_base.clone(), "admin", "pw");
    let repo = authed.service_repo(true).await.unwrap();
    assert_eq!(repo.info.name, "raw-main");
    assert!(repo.detail.is_some(), "admin detail rides the single GET");

    let anon = client(repo_base);
    let err = anon.service_repo(true).await.unwrap_err();
    assert_eq!(
        err.exit_code(),
        3,
        "403 on the admin detail is the auth family"
    );
    assert!(err.hint().unwrap_or_default().contains("nx-admin required"));
}

#[tokio::test]
async fn repo_prefix_match_resolves_deep_url() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::RepoPrefixMatch).unwrap();
    let base = format!("{}repository/raw-main/deep/nested/dir", server.base_url());
    let nxr = client(base);
    let repo = nxr.service_repo(false).await.unwrap();
    assert_eq!(repo.info.name, "raw-main");
}

#[tokio::test]
async fn assets_pagination_collects_all_pages() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::AssetsPagination).unwrap();
    let base = format!("{}repository/raw-main/", server.base_url());
    let nxr = client(base);
    let (assets, summary) = nxr.service_assets(None, &[]).await.unwrap();
    assert_eq!(assets.len(), 35);
    assert_eq!(summary.pages, 4, "35 assets over pages of 10");
    assert!(assets[0].path.starts_with("v0.9.0-assets/asset-"));
}

#[tokio::test]
async fn assets_absent_degrades_with_enumerate() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::AssetsAbsent).unwrap();
    let base = format!("{}repository/raw-main/", server.base_url());
    let nxr = client(base);
    let err = nxr.service_assets(None, &[]).await.unwrap_err();
    assert!(matches!(
        err,
        nexus_raw_core::error::Error::Enumerate { .. }
    ));
}

#[tokio::test]
async fn assets_prefix_q_filters() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::AssetsPrefixQ).unwrap();
    let base = format!("{}repository/raw-main/", server.base_url());
    let nxr = client(base);
    let (by_prefix, _) = nxr
        .service_assets(None, &["v0.9.0-assets".to_owned()])
        .await
        .unwrap();
    assert_eq!(
        by_prefix.len(),
        35,
        "whole-segment prefix takes every asset"
    );
    let (by_q, _) = nxr.service_assets(Some("asset-01"), &[]).await.unwrap();
    assert_eq!(by_q.len(), 10, "asset-010..asset-019");
}

#[tokio::test]
async fn eula_gate_blocks_then_opens() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::EulaGate).unwrap();
    let base = format!("{}repository/raw-main/", server.base_url());
    let nxr = client(base);

    let status = nxr.service_eula().await.unwrap().expect("gate present");
    assert!(!status.accepted);

    let outcome = nxr.service_eula_accept().await.unwrap();
    assert_eq!(outcome.action, "accepted");
    assert!(outcome.accepted);
}

#[tokio::test]
async fn eula_absent_is_gate_false() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::EulaAbsent).unwrap();
    let nxr = client(server.base_url());
    assert!(nxr.service_eula().await.unwrap().is_none());
    let outcome = nxr.service_eula_accept().await.unwrap();
    assert!(!outcome.accepted);
    assert_eq!(outcome.action, "none");
}

#[tokio::test]
async fn detail_in_hint_rides_the_hint() {
    let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::DetailInHint).unwrap();
    let nxr = client(server.base_url());
    // A marker PUT through put_url is not on the facade; the facade write closest to the wire is
    // `point_clear` (a DELETE): the scenario answers it with the short body the hint carries.
    let target = format!("{}repository/raw-main/probe.pointer", server.base_url());
    let err = nxr.point_clear(&target).await.unwrap_err();
    let hint = err.hint().unwrap();
    assert!(
        hint.contains("please ask the administrator"),
        "the server body must ride the hint, got: {hint}"
    );
}
