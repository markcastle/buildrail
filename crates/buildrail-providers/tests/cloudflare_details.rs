//! Cloudflare build details against a local mock server: read from the
//! builds list, reusing a page the history just fetched.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use buildrail_core::details::{RunRef, Trigger};
use buildrail_core::domain::{ProviderKind, RunId, WatchTarget};
use buildrail_core::ports::{ProviderError, RunDetailsSource, RunSource};
use buildrail_providers::cloudflare::CloudflareBuilds;
use common::{assert_no_token, client, fixture, secret, TestClock};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACCOUNT: &str = "023e105f4ecef8ad9ca31a8372d0c353";
const TAG: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d";
const DEPLOYED: &str = "5e0c1a2b-0000-4000-8000-000000000001";

fn target() -> WatchTarget {
    WatchTarget::Cloudflare {
        account_id: ACCOUNT.into(),
        worker: "my-worker".into(),
        branch: None,
    }
}

fn run(uuid: &str) -> RunRef {
    RunRef {
        run_id: RunId::new(uuid),
        attempt: 1,
    }
}

fn ok_json(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(fixture(name), "application/json")
}

async fn mount(server: &MockServer, per_page: &str, builds: u64) {
    Mock::given(method("GET"))
        .and(path(format!(
            "/client/v4/accounts/{ACCOUNT}/workers/scripts"
        )))
        .respond_with(ok_json("cloudflare/scripts.json"))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/client/v4/accounts/{ACCOUNT}/builds/workers/{TAG}/builds"
        )))
        .and(query_param("per_page", per_page))
        .respond_with(ok_json("cloudflare/builds.json"))
        .expect(builds)
        .mount(server)
        .await;
}

#[tokio::test]
async fn details_come_from_the_list_the_history_just_fetched() {
    let server = MockServer::start().await;
    // One scripts call, one list: the history and the details share it.
    mount(&server, "10", 1).await;
    let clock = TestClock::new();
    let source = CloudflareBuilds::new(client(), clock.clone())
        .with_base_url(format!("{}/client/v4", server.uri()));
    assert_eq!(
        RunDetailsSource::provider(&source),
        ProviderKind::Cloudflare
    );
    let history = source.recent_runs(&target(), &secret()).await.unwrap();
    assert_eq!(history.len(), 4);
    clock.advance(3);
    let details = source
        .run_details(&target(), &run(DEPLOYED), &secret())
        .await
        .unwrap();
    assert_eq!(details.run, run(DEPLOYED));
    assert_eq!(details.commit_message.as_deref(), Some("Add new feature"));
    assert_eq!(details.actor.as_deref(), Some("developer@example.com"));
    assert_eq!(details.trigger, Trigger::Push);
    assert_eq!(
        details.started_at.unwrap().to_rfc3339(),
        "2026-11-16T10:00:05+00:00"
    );
    assert_eq!(
        details.finished_at.unwrap().to_rfc3339(),
        "2026-11-16T10:02:00+00:00"
    );
    assert_eq!(details.jobs, None, "Workers Builds lists no stages");
    assert_eq!(details.title, None);
}

#[tokio::test]
async fn an_old_list_is_not_reused_and_a_missing_build_is_not_found() {
    let server = MockServer::start().await;
    mount(&server, "10", 1).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/client/v4/accounts/{ACCOUNT}/builds/workers/{TAG}/builds"
        )))
        .and(query_param("per_page", "50"))
        .respond_with(ok_json("cloudflare/builds.json"))
        .expect(2)
        .mount(&server)
        .await;
    let clock = TestClock::new();
    let source = CloudflareBuilds::new(client(), clock.clone())
        .with_base_url(format!("{}/client/v4", server.uri()));
    source.recent_runs(&target(), &secret()).await.unwrap();
    // A minute later the page is too old: listed again (a bigger page).
    clock.advance(60);
    let running = source
        .run_details(
            &target(),
            &run("5e0c1a2b-0000-4000-8000-000000000004"),
            &secret(),
        )
        .await
        .unwrap();
    assert_eq!(running.finished_at, None);
    assert!(running.started_at.is_some());
    // A build that is not in the list (older than a page, or deleted).
    clock.advance(60);
    let error = source
        .run_details(&target(), &run("nope"), &secret())
        .await
        .unwrap_err();
    assert_eq!(
        error,
        ProviderError::NotFound {
            provider: ProviderKind::Cloudflare
        }
    );
    assert_no_token(&error);
}

#[tokio::test]
async fn errors_and_wrong_targets() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/client/v4/accounts/{ACCOUNT}/workers/scripts"
        )))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "5"))
        .mount(&server)
        .await;
    let source = CloudflareBuilds::new(client(), TestClock::new())
        .with_base_url(format!("{}/client/v4", server.uri()));
    let error = source
        .run_details(&target(), &run(DEPLOYED), &secret())
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProviderError::RateLimited { .. }),
        "{error}"
    );
    assert_no_token(&error);
    let gh = WatchTarget::GitHub {
        owner: "o".into(),
        repo: "r".into(),
        workflow: "w".into(),
        branch: None,
    };
    assert_eq!(
        source
            .run_details(&gh, &run(DEPLOYED), &secret())
            .await
            .unwrap_err(),
        ProviderError::NotFound {
            provider: ProviderKind::Cloudflare
        }
    );
}
