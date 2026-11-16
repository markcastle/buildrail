//! Cloudflare adapter tests against a local mock server.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use buildrail_core::domain::{BuildStatus, ProviderKind, WatchTarget};
use buildrail_core::ports::{Clock, ProviderError, RunSource};
use buildrail_providers::catalog::{CloudflareAccounts, CloudflareTokenCheck, CloudflareWorkers};
use buildrail_providers::cloudflare::{CloudflareBuilds, CloudflareCatalog};
use buildrail_providers::USER_AGENT;
use common::{assert_no_token, client, fixture, json, secret, TestClock, TOKEN};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACCOUNT: &str = "023e105f4ecef8ad9ca31a8372d0c353";
const TAG: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d";

fn scripts_path() -> String {
    format!("/client/v4/accounts/{ACCOUNT}/workers/scripts")
}

fn builds_path(tag: &str) -> String {
    format!("/client/v4/accounts/{ACCOUNT}/builds/workers/{tag}/builds")
}

fn target(branch: Option<&str>) -> WatchTarget {
    WatchTarget::Cloudflare {
        account_id: ACCOUNT.into(),
        worker: "my-worker".into(),
        branch: branch.map(str::to_owned),
    }
}

fn base(server: &MockServer) -> String {
    format!("{}/client/v4", server.uri())
}

fn adapter(server: &MockServer) -> CloudflareBuilds {
    CloudflareBuilds::new(client(), TestClock::new()).with_base_url(base(server))
}

fn ok_json(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(fixture(name), "application/json")
}

async fn mount_scripts(server: &MockServer, times: u64) {
    Mock::given(method("GET"))
        .and(path(scripts_path()))
        .respond_with(ok_json("cloudflare/scripts.json"))
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test]
async fn resolves_tag_lists_builds_and_maps_statuses() {
    let server = MockServer::start().await;
    mount_scripts(&server, 1).await;
    Mock::given(method("GET"))
        .and(path(builds_path(TAG)))
        .and(query_param("page", "1"))
        .and(query_param("per_page", "10"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(header("accept", "application/json"))
        .and(header("user-agent", USER_AGENT))
        .respond_with(ok_json("cloudflare/builds.json"))
        .expect(2)
        .mount(&server)
        .await;

    let clock = TestClock::new();
    let source = CloudflareBuilds::new(client(), clock.clone()).with_base_url(base(&server));
    assert_eq!(source.provider(), ProviderKind::Cloudflare);
    let builds = source.recent_runs(&target(None), &secret()).await.unwrap();

    let summary: Vec<_> = builds
        .iter()
        .map(|b| (b.run_id.as_str(), b.status))
        .collect();
    assert_eq!(
        summary,
        [
            ("5e0c1a2b-0000-4000-8000-000000000004", BuildStatus::Running),
            (
                "5e0c1a2b-0000-4000-8000-000000000001",
                BuildStatus::Deployed
            ),
            ("5e0c1a2b-0000-4000-8000-000000000002", BuildStatus::Passed),
            ("5e0c1a2b-0000-4000-8000-000000000003", BuildStatus::Failed),
        ]
    );
    let deployed = &builds[1];
    assert_eq!(deployed.attempt, 1);
    assert_eq!(deployed.branch.as_deref(), Some("main"));
    assert_eq!(deployed.commit.as_deref(), Some("abc123def456"));
    assert_eq!(deployed.failing_job, None);
    assert_eq!(
        deployed.url,
        format!(
            "https://dash.cloudflare.com/{ACCOUNT}/workers/services/view/my-worker/production/builds/5e0c1a2b-0000-4000-8000-000000000001"
        )
    );
    assert_eq!(
        deployed.created_at.to_rfc3339(),
        "2026-11-16T10:00:00+00:00"
    );
    assert_eq!(
        deployed.updated_at.to_rfc3339(),
        "2026-11-16T10:02:00.500+00:00"
    );
    assert!(builds.iter().all(|b| b.observed_at == clock.now()));
    // Running build has no modified_on: falls back to created_on.
    assert_eq!(builds[0].updated_at, builds[0].created_at);

    // Second poll reuses the cached tag (scripts expected once).
    clock.advance(30);
    let again = source.recent_runs(&target(None), &secret()).await.unwrap();
    assert_eq!(again[0].observed_at, clock.now());
}

#[tokio::test]
async fn branch_filter_is_applied_locally_with_bigger_page() {
    let server = MockServer::start().await;
    mount_scripts(&server, 1).await;
    Mock::given(method("GET"))
        .and(path(builds_path(TAG)))
        .and(query_param("per_page", "50"))
        .respond_with(ok_json("cloudflare/builds.json"))
        .expect(1)
        .mount(&server)
        .await;
    let builds = adapter(&server)
        .with_build_limit(1)
        .recent_runs(&target(Some("main")), &secret())
        .await
        .unwrap();
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].branch.as_deref(), Some("main"));
    assert_eq!(builds[0].status, BuildStatus::Deployed);
}

#[tokio::test]
async fn empty_history_is_ok_and_empty() {
    let server = MockServer::start().await;
    mount_scripts(&server, 1).await;
    Mock::given(method("GET"))
        .and(path(builds_path(TAG)))
        .respond_with(ok_json("cloudflare/builds_empty.json"))
        .mount(&server)
        .await;
    let builds = adapter(&server)
        .recent_runs(&target(None), &secret())
        .await
        .unwrap();
    assert!(builds.is_empty());
}

#[tokio::test]
async fn null_result_is_empty() {
    let server = MockServer::start().await;
    mount_scripts(&server, 1).await;
    Mock::given(method("GET"))
        .and(path(builds_path(TAG)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "errors": [], "messages": [], "result": null
        })))
        .mount(&server)
        .await;
    assert!(adapter(&server)
        .recent_runs(&target(None), &secret())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn unknown_worker_is_not_found() {
    let server = MockServer::start().await;
    mount_scripts(&server, 1).await;
    let missing = WatchTarget::Cloudflare {
        account_id: ACCOUNT.into(),
        worker: "nope".into(),
        branch: None,
    };
    assert_eq!(
        adapter(&server)
            .recent_runs(&missing, &secret())
            .await
            .unwrap_err(),
        ProviderError::NotFound {
            provider: ProviderKind::Cloudflare
        }
    );
}

#[tokio::test]
async fn stale_cached_tag_is_resolved_again_once() {
    let server = MockServer::start().await;
    const NEW_TAG: &str = "99999999999999999999999999999999";
    let mut recreated = json("cloudflare/scripts.json");
    recreated["result"][1]["tag"] = NEW_TAG.into();

    // First scripts call: old tag; later calls: new tag.
    Mock::given(method("GET"))
        .and(path(scripts_path()))
        .respond_with(ok_json("cloudflare/scripts.json"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(scripts_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(&recreated))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(builds_path(TAG)))
        .respond_with(ok_json("cloudflare/builds.json"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(builds_path(TAG)))
        .respond_with(ResponseTemplate::new(404).set_body_raw(
            fixture("cloudflare/error_not_found.json"),
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(builds_path(NEW_TAG)))
        .respond_with(ok_json("cloudflare/builds_empty.json"))
        .expect(1)
        .mount(&server)
        .await;

    let source = adapter(&server);
    assert_eq!(
        source
            .recent_runs(&target(None), &secret())
            .await
            .unwrap()
            .len(),
        4
    );
    assert!(source
        .recent_runs(&target(None), &secret())
        .await
        .unwrap()
        .is_empty());
}

async fn error_for(response: ResponseTemplate) -> ProviderError {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(scripts_path()))
        .respond_with(response)
        .mount(&server)
        .await;
    let err = adapter(&server)
        .recent_runs(&target(None), &secret())
        .await
        .unwrap_err();
    assert_no_token(&err);
    err
}

fn body(name: &str, status: u16) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_raw(fixture(name), "application/json")
}

#[tokio::test]
async fn error_classes() {
    let cf = ProviderKind::Cloudflare;
    assert_eq!(
        error_for(body("cloudflare/error_auth.json", 403)).await,
        ProviderError::AccessDenied { provider: cf }
    );
    assert_eq!(
        error_for(body("cloudflare/error_auth.json", 401)).await,
        ProviderError::AccessDenied { provider: cf }
    );
    assert_eq!(
        error_for(body("cloudflare/error_bad_header.json", 400)).await,
        ProviderError::AccessDenied { provider: cf }
    );
    assert_eq!(
        error_for(body("cloudflare/error_not_found.json", 400)).await,
        ProviderError::Server {
            provider: cf,
            status: 400
        }
    );
    assert_eq!(
        error_for(body("cloudflare/error_not_found.json", 404)).await,
        ProviderError::NotFound { provider: cf }
    );
    assert_eq!(
        error_for(ResponseTemplate::new(503)).await,
        ProviderError::Server {
            provider: cf,
            status: 503
        }
    );
    assert_eq!(
        error_for(ResponseTemplate::new(429).insert_header("retry-after", "30")).await,
        ProviderError::RateLimited {
            provider: cf,
            retry_after: Some(Duration::from_secs(30))
        }
    );
    assert!(matches!(
        error_for(body("cloudflare/error_auth.json", 200)).await,
        ProviderError::Malformed(_)
    ));
    assert!(matches!(
        error_for(ResponseTemplate::new(200).set_body_string("nope")).await,
        ProviderError::Malformed(_)
    ));
}

#[tokio::test]
async fn builds_errors_propagate() {
    let server = MockServer::start().await;
    mount_scripts(&server, 1).await;
    Mock::given(method("GET"))
        .and(path(builds_path(TAG)))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    assert_eq!(
        adapter(&server)
            .recent_runs(&target(None), &secret())
            .await
            .unwrap_err(),
        ProviderError::AccessDenied {
            provider: ProviderKind::Cloudflare
        }
    );
}

#[tokio::test]
async fn network_error_and_debug_do_not_leak_token() {
    let source = CloudflareBuilds::new(client(), TestClock::new())
        .with_base_url("http://127.0.0.1:1/client/v4");
    let err = source
        .recent_runs(&target(None), &secret())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Network(_)), "{err:?}");
    assert_no_token(&err);
    assert_no_token(&format!("{source:?}"));
}

#[tokio::test]
async fn wrong_provider_target_is_not_found() {
    let gh = WatchTarget::GitHub {
        owner: "o".into(),
        repo: "r".into(),
        workflow: "ci.yml".into(),
        branch: None,
    };
    assert_eq!(
        CloudflareBuilds::new(client(), TestClock::new())
            .recent_runs(&gh, &secret())
            .await
            .unwrap_err(),
        ProviderError::NotFound {
            provider: ProviderKind::Cloudflare
        }
    );
}

// ------------------------------------------------------------- catalogue ---

fn catalog(server: &MockServer) -> CloudflareCatalog {
    CloudflareCatalog::new(client(), TestClock::new()).with_base_url(base(server))
}

#[tokio::test]
async fn verify_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ok_json("cloudflare/token_verify.json"))
        .mount(&server)
        .await;
    let status = catalog(&server).verify_token(&secret()).await.unwrap();
    assert!(status.active);
    assert_eq!(status.status, "active");
    assert_eq!(status.id, "ed17574386854bf78a67040be0a770b0");
    assert_eq!(
        status.expires_on.unwrap().to_rfc3339(),
        "2027-01-01T00:00:00+00:00"
    );
}

#[tokio::test]
async fn verify_token_failures() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "errors": [], "messages": [], "result": null
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(body("cloudflare/error_bad_header.json", 400))
        .mount(&server)
        .await;
    let catalog = catalog(&server);
    assert!(matches!(
        catalog.verify_token(&secret()).await.unwrap_err(),
        ProviderError::Malformed(_)
    ));
    let err = catalog.verify_token(&secret()).await.unwrap_err();
    assert_eq!(
        err,
        ProviderError::AccessDenied {
            provider: ProviderKind::Cloudflare
        }
    );
    assert_no_token(&err);
}

#[tokio::test]
async fn accounts_paginate_until_short_page() {
    let server = MockServer::start().await;
    let full: Vec<_> = (0..50)
        .map(|i| serde_json::json!({"id": format!("{i:032x}"), "name": format!("Account {i}")}))
        .collect();
    Mock::given(method("GET"))
        .and(path("/client/v4/accounts"))
        .and(query_param("page", "1"))
        .and(query_param("per_page", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "errors": [], "messages": [], "result": full,
            "result_info": {"page": 1, "per_page": 50, "count": 50, "total_count": 51}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/client/v4/accounts"))
        .and(query_param("page", "2"))
        .respond_with(ok_json("cloudflare/accounts_page1.json"))
        .expect(1)
        .mount(&server)
        .await;
    let accounts = catalog(&server).accounts(&secret()).await.unwrap();
    assert_eq!(accounts.len(), 51);
    assert_eq!(accounts[50].name, "Demo Account");
    assert_eq!(accounts[50].id, ACCOUNT);
}

#[tokio::test]
async fn accounts_stop_on_total_pages() {
    let server = MockServer::start().await;
    let full: Vec<_> = (0..50)
        .map(|i| serde_json::json!({"id": format!("{i:032x}"), "name": "x"}))
        .collect();
    Mock::given(method("GET"))
        .and(path("/client/v4/accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "errors": [], "messages": [], "result": full,
            "result_info": {"page": 1, "per_page": 50, "total_pages": 1}
        })))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        catalog(&server).accounts(&secret()).await.unwrap().len(),
        50
    );
}

#[tokio::test]
async fn accounts_are_capped() {
    let server = MockServer::start().await;
    let full: Vec<_> = (0..50)
        .map(|i| serde_json::json!({"id": format!("{i:032x}"), "name": "x"}))
        .collect();
    Mock::given(method("GET"))
        .and(path("/client/v4/accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "errors": [], "messages": [], "result": full
        })))
        .expect(10)
        .mount(&server)
        .await;
    assert_eq!(
        catalog(&server).accounts(&secret()).await.unwrap().len(),
        buildrail_providers::catalog::MAX_ACCOUNTS
    );
}

#[tokio::test]
async fn workers_list() {
    let server = MockServer::start().await;
    mount_scripts(&server, 1).await;
    let workers = catalog(&server).workers(ACCOUNT, &secret()).await.unwrap();
    assert_eq!(workers.len(), 2);
    assert_eq!(workers[1].name, "my-worker");
    assert_eq!(workers[1].tag.as_deref(), Some(TAG));
    assert!(workers[1].modified_on.is_some());
}

#[tokio::test]
async fn workers_list_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(scripts_path()))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let catalog = catalog(&server);
    assert_eq!(
        catalog.workers(ACCOUNT, &secret()).await.unwrap_err(),
        ProviderError::RateLimited {
            provider: ProviderKind::Cloudflare,
            retry_after: None
        }
    );
    assert_no_token(&format!("{catalog:?}"));
}
