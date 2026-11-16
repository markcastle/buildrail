//! GitHub adapter tests against a local mock server.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use buildrail_core::domain::{BuildStatus, ProviderKind, WatchTarget};
use buildrail_core::ports::{ProviderError, RunSource};
use buildrail_providers::catalog::{
    GitHubBranches, GitHubIdentity, GitHubRepositories, GitHubWorkflows,
};
use buildrail_providers::github::{GitHubActions, GitHubCatalog, GITHUB_API_VERSION};
use buildrail_providers::USER_AGENT;
use common::{assert_no_token, client, fixture, secret, TestClock, TOKEN};
use wiremock::matchers::{
    header, header_exists, method, path, query_param, query_param_is_missing,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

const RUNS_PATH: &str = "/repos/octo-org/octo-repo/actions/workflows/build.yml/runs";
const JOBS_PATH: &str = "/repos/octo-org/octo-repo/actions/runs/30433642/attempts/2/jobs";

fn target(branch: Option<&str>) -> WatchTarget {
    WatchTarget::GitHub {
        owner: "octo-org".into(),
        repo: "octo-repo".into(),
        workflow: "build.yml".into(),
        branch: branch.map(str::to_owned),
    }
}

fn adapter(server: &MockServer, clock: Arc<TestClock>) -> GitHubActions {
    GitHubActions::new(client(), clock).with_base_url(server.uri())
}

fn ok_json(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(fixture(name), "application/json")
}

#[tokio::test]
async fn maps_runs_sends_headers_and_branch_and_names_failing_job() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .and(query_param("branch", "main"))
        .and(query_param("per_page", "10"))
        .and(query_param("exclude_pull_requests", "true"))
        .and(header("accept", "application/vnd.github+json"))
        .and(header("x-github-api-version", GITHUB_API_VERSION))
        .and(header("user-agent", USER_AGENT))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ok_json("github/workflow_runs.json"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(JOBS_PATH))
        .and(query_param("per_page", "100"))
        .respond_with(ok_json("github/jobs.json"))
        .expect(1)
        .mount(&server)
        .await;

    let clock = TestClock::new();
    let source = adapter(&server, clock.clone());
    assert_eq!(source.provider(), ProviderKind::GitHub);
    let runs = source
        .recent_runs(&target(Some("main")), &secret())
        .await
        .unwrap();

    assert_eq!(runs.len(), 3);
    let failed = &runs[0];
    assert_eq!(failed.run_id.as_str(), "30433642");
    assert_eq!(failed.attempt, 2);
    assert_eq!(failed.status, BuildStatus::Failed);
    assert_eq!(failed.failing_job.as_deref(), Some("test (ubuntu-latest)"));
    assert_eq!(
        failed.url,
        "https://github.com/octo-org/octo-repo/actions/runs/30433642"
    );
    assert_eq!(failed.branch.as_deref(), Some("main"));
    assert_eq!(
        failed.commit.as_deref(),
        Some("acb5820ced9479c074f688cc328bf03f341a511d")
    );
    assert_eq!(failed.created_at.to_rfc3339(), "2026-11-16T10:05:00+00:00");
    assert_eq!(failed.updated_at.to_rfc3339(), "2026-11-16T10:09:30+00:00");
    assert_eq!(runs[1].status, BuildStatus::Passed);
    assert_eq!(runs[1].failing_job, None);
    assert_eq!(runs[2].status, BuildStatus::Running);
    assert!(runs.iter().all(|r| r.observed_at == clock_now(&clock)));
}

fn clock_now(clock: &TestClock) -> chrono::DateTime<chrono::Utc> {
    use buildrail_core::ports::Clock;
    clock.now()
}

#[tokio::test]
async fn no_branch_filter_means_no_branch_param_and_no_jobs_call_when_not_failed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .and(query_param_is_missing("branch"))
        .and(query_param("per_page", "5"))
        .respond_with(ok_json("github/workflow_runs_passing.json"))
        .expect(1)
        .mount(&server)
        .await;
    let source = adapter(&server, TestClock::new()).with_run_limit(5);
    let runs = source.recent_runs(&target(None), &secret()).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, BuildStatus::Queued);
    // Only the runs mock exists: an unexpected jobs call would 404 and the
    // expectation above verifies the single request.
}

#[tokio::test]
async fn empty_history_is_ok_and_empty() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .respond_with(ok_json("github/workflow_runs_empty.json"))
        .mount(&server)
        .await;
    let runs = adapter(&server, TestClock::new())
        .recent_runs(&target(None), &secret())
        .await
        .unwrap();
    assert!(runs.is_empty());
}

#[tokio::test]
async fn etag_304_reuses_cached_records_with_fresh_observed_at() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .and(header("if-none-match", "W/\"runs-v1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .respond_with(ok_json("github/workflow_runs.json").insert_header("etag", "W/\"runs-v1\""))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(JOBS_PATH))
        .respond_with(ok_json("github/jobs.json"))
        .expect(1) // failing job is cached with the runs
        .mount(&server)
        .await;

    let clock = TestClock::new();
    let source = adapter(&server, clock.clone());
    let first = source.recent_runs(&target(None), &secret()).await.unwrap();
    clock.advance(60);
    let second = source.recent_runs(&target(None), &secret()).await.unwrap();
    clock.advance(60);
    let third = source.recent_runs(&target(None), &secret()).await.unwrap();

    assert_eq!(first.len(), second.len());
    assert_eq!(
        second[0].failing_job.as_deref(),
        Some("test (ubuntu-latest)")
    );
    assert_eq!(
        third[0].observed_at - first[0].observed_at,
        chrono::Duration::seconds(120)
    );
    let strip = |mut r: buildrail_core::domain::BuildRecord| {
        r.observed_at = first[0].observed_at;
        r
    };
    assert_eq!(
        third.into_iter().map(strip).collect::<Vec<_>>(),
        first.clone()
    );
}

#[tokio::test]
async fn etag_is_not_shared_between_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .and(header_exists("if-none-match"))
        .respond_with(ResponseTemplate::new(304))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .respond_with(ok_json("github/workflow_runs_passing.json").insert_header("etag", "\"x\""))
        .expect(2)
        .mount(&server)
        .await;
    let source = adapter(&server, TestClock::new());
    source.recent_runs(&target(None), &secret()).await.unwrap();
    source
        .recent_runs(
            &target(None),
            &buildrail_core::ports::Secret::new("another-token"),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn failing_job_lookup_failure_is_tolerated_and_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .respond_with(ok_json("github/workflow_runs.json").insert_header("etag", "\"v1\""))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(JOBS_PATH))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(JOBS_PATH))
        .respond_with(ok_json("github/jobs.json"))
        .expect(1)
        .mount(&server)
        .await;

    let source = adapter(&server, TestClock::new());
    let first = source.recent_runs(&target(None), &secret()).await.unwrap();
    assert_eq!(first[0].status, BuildStatus::Failed);
    assert_eq!(first[0].failing_job, None);

    // Next poll (304) retries the lookup once and then stops asking.
    let second = source.recent_runs(&target(None), &secret()).await.unwrap();
    assert_eq!(
        second[0].failing_job.as_deref(),
        Some("test (ubuntu-latest)")
    );
    let third = source.recent_runs(&target(None), &secret()).await.unwrap();
    assert_eq!(
        third[0].failing_job.as_deref(),
        Some("test (ubuntu-latest)")
    );
}

#[tokio::test]
async fn failing_job_malformed_jobs_body_is_tolerated() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .respond_with(ok_json("github/workflow_runs.json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(JOBS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("{not json"))
        .mount(&server)
        .await;
    let runs = adapter(&server, TestClock::new())
        .recent_runs(&target(None), &secret())
        .await
        .unwrap();
    assert_eq!(runs[0].failing_job, None);
}

async fn error_for(response: ResponseTemplate) -> ProviderError {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(RUNS_PATH))
        .respond_with(response)
        .mount(&server)
        .await;
    let err = adapter(&server, TestClock::new())
        .recent_runs(&target(Some("main")), &secret())
        .await
        .unwrap_err();
    assert_no_token(&err);
    err
}

#[tokio::test]
async fn error_classes() {
    let gh = ProviderKind::GitHub;
    assert_eq!(
        error_for(ResponseTemplate::new(401).set_body_raw(
            fixture("github/error_bad_credentials.json"),
            "application/json"
        ))
        .await,
        ProviderError::AccessDenied { provider: gh }
    );
    assert_eq!(
        error_for(ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "4999")).await,
        ProviderError::AccessDenied { provider: gh }
    );
    assert_eq!(
        error_for(ResponseTemplate::new(404)).await,
        ProviderError::NotFound { provider: gh }
    );
    assert_eq!(
        error_for(ResponseTemplate::new(500)).await,
        ProviderError::Server {
            provider: gh,
            status: 500
        }
    );
    assert_eq!(
        error_for(ResponseTemplate::new(422)).await,
        ProviderError::Server {
            provider: gh,
            status: 422
        }
    );
    assert!(matches!(
        error_for(ResponseTemplate::new(200).set_body_string("<html>")).await,
        ProviderError::Malformed(_)
    ));
    assert!(matches!(
        error_for(ResponseTemplate::new(200).set_body_string(r#"{"workflow_runs":[{"id":"x"}]}"#))
            .await,
        ProviderError::Malformed(_)
    ));
}

#[tokio::test]
async fn rate_limit_variants() {
    let gh = ProviderKind::GitHub;
    // Primary limit: 403 + remaining 0, wait computed from the reset epoch
    // against the injected clock (12:00:00 + 90 s).
    let clock_now = TestClock::new();
    let reset = {
        use buildrail_core::ports::Clock;
        clock_now.now().timestamp() + 90
    };
    assert_eq!(
        error_for(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset", reset.to_string().as_str())
        )
        .await,
        ProviderError::RateLimited {
            provider: gh,
            retry_after: Some(Duration::from_secs(90))
        }
    );
    // Secondary limit: 403 + retry-after.
    assert_eq!(
        error_for(ResponseTemplate::new(403).insert_header("retry-after", "60")).await,
        ProviderError::RateLimited {
            provider: gh,
            retry_after: Some(Duration::from_secs(60))
        }
    );
    // 429 without hints.
    assert_eq!(
        error_for(ResponseTemplate::new(429)).await,
        ProviderError::RateLimited {
            provider: gh,
            retry_after: None
        }
    );
}

#[tokio::test]
async fn network_error_does_not_leak_token() {
    // Nothing listens on port 1.
    let source = GitHubActions::new(client(), TestClock::new()).with_base_url("http://127.0.0.1:1");
    let err = source
        .recent_runs(&target(None), &secret())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Network(_)), "{err:?}");
    assert_no_token(&err);
    assert_no_token(&format!("{source:?}"));
}

#[tokio::test]
async fn invalid_base_url_is_reported() {
    let source = GitHubActions::new(client(), TestClock::new()).with_base_url("::nope::");
    let err = source
        .recent_runs(&target(None), &secret())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Malformed(_)));
}

#[tokio::test]
async fn wrong_provider_target_is_not_found() {
    let source = GitHubActions::new(client(), TestClock::new());
    let cf = WatchTarget::Cloudflare {
        account_id: "a".into(),
        worker: "w".into(),
        branch: None,
    };
    assert_eq!(
        source.recent_runs(&cf, &secret()).await.unwrap_err(),
        ProviderError::NotFound {
            provider: ProviderKind::GitHub
        }
    );
}

// ------------------------------------------------------------- catalogue ---

fn catalog(server: &MockServer) -> GitHubCatalog {
    GitHubCatalog::new(client(), TestClock::new()).with_base_url(server.uri())
}

#[tokio::test]
async fn current_user() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ok_json("github/user.json"))
        .mount(&server)
        .await;
    let user = catalog(&server).current_user(&secret()).await.unwrap();
    assert_eq!(user.login, "octocat");
    assert_eq!(user.name.as_deref(), Some("monalisa octocat"));
}

#[tokio::test]
async fn current_user_bad_token_is_access_denied() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let err = catalog(&server).current_user(&secret()).await.unwrap_err();
    assert_eq!(
        err,
        ProviderError::AccessDenied {
            provider: ProviderKind::GitHub
        }
    );
    assert_no_token(&err);
}

#[tokio::test]
async fn repositories_follow_link_pagination_on_same_host_only() {
    let server = MockServer::start().await;
    let next = format!(
        "{}/user/repos?per_page=100&sort=pushed&page=2",
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("page", "2"))
        .respond_with(ok_json("github/repos_page2.json").insert_header(
            "link",
            "<https://evil.example/user/repos?page=3>; rel=\"next\"",
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("per_page", "100"))
        .and(query_param("sort", "pushed"))
        .and(query_param_is_missing("page"))
        .respond_with(ok_json("github/repos_page1.json").insert_header(
            "link",
            format!("<{next}>; rel=\"next\", <{next}>; rel=\"last\"").as_str(),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let repos = catalog(&server).repositories(&secret()).await.unwrap();
    let names: Vec<_> = repos.iter().map(|r| r.full_name.as_str()).collect();
    assert_eq!(
        names,
        [
            "octocat/Hello-World",
            "octo-org/secret-sauce",
            "octocat/old-thing"
        ]
    );
    assert_eq!(repos[1].owner, "octo-org");
    assert_eq!(repos[1].name, "secret-sauce");
    assert!(repos[1].private);
    assert!(repos[2].archived);
    assert_eq!(repos[0].default_branch.as_deref(), Some("master"));
}

#[tokio::test]
async fn repositories_are_capped() {
    let server = MockServer::start().await;
    let page: Vec<_> = (0..100)
        .map(|i| {
            serde_json::json!({
                "name": format!("r{i}"), "full_name": format!("o/r{i}"),
                "owner": {"login": "o"}, "private": false, "archived": false,
                "default_branch": "main"
            })
        })
        .collect();
    // Every page points to itself as "next": the cap must stop the loop.
    let next = format!("{}/user/repos?page=2", server.uri());
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(&page)
                .insert_header("link", format!("<{next}>; rel=\"next\"").as_str()),
        )
        .expect(10)
        .mount(&server)
        .await;
    let repos = catalog(&server).repositories(&secret()).await.unwrap();
    assert_eq!(repos.len(), buildrail_providers::catalog::MAX_REPOSITORIES);
}

#[tokio::test]
async fn workflows_and_watch_keys() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octo-org/octo-repo/actions/workflows"))
        .and(query_param("per_page", "100"))
        .respond_with(ok_json("github/workflows.json"))
        .mount(&server)
        .await;
    let workflows = catalog(&server)
        .workflows("octo-org", "octo-repo", &secret())
        .await
        .unwrap();
    assert_eq!(workflows.len(), 2);
    assert_eq!(workflows[0].name, "CI");
    assert_eq!(workflows[0].watch_key(), "blank.yaml");
    assert_eq!(workflows[1].watch_key(), "269289");
    assert_eq!(workflows[0].state, "active");
}

#[tokio::test]
async fn branches() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octocat/Hello-World/branches"))
        .and(query_param("per_page", "100"))
        .respond_with(ok_json("github/branches.json"))
        .mount(&server)
        .await;
    let branches = catalog(&server)
        .branches("octocat", "Hello-World", &secret())
        .await
        .unwrap();
    assert_eq!(branches.len(), 2);
    assert_eq!(branches[0].name, "master");
    assert!(branches[0].protected);
    assert_eq!(branches[1].name, "feature/login");
}

#[tokio::test]
async fn catalog_errors_map_like_run_source() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/o/missing/branches"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/o/r/actions/workflows"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "7"))
        .mount(&server)
        .await;
    let catalog = catalog(&server);
    assert_eq!(
        catalog
            .branches("o", "missing", &secret())
            .await
            .unwrap_err(),
        ProviderError::NotFound {
            provider: ProviderKind::GitHub
        }
    );
    assert_eq!(
        catalog.workflows("o", "r", &secret()).await.unwrap_err(),
        ProviderError::RateLimited {
            provider: ProviderKind::GitHub,
            retry_after: Some(Duration::from_secs(7))
        }
    );
    assert_no_token(&format!("{catalog:?}"));
}
