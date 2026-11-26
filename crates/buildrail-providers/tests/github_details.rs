//! GitHub run details against a local mock server: the attempt, its jobs
//! (paginated and capped), failing steps and every error class.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use buildrail_core::details::{RunRef, Trigger};
use buildrail_core::domain::{BuildStatus, ProviderKind, RunId, WatchTarget};
use buildrail_core::ports::{ProviderError, RunDetailsSource};
use buildrail_providers::github::{GitHubActions, GITHUB_API_VERSION, MAX_JOB_PAGES};
use buildrail_providers::USER_AGENT;
use common::{assert_no_token, client, fixture, secret, TestClock, TOKEN};
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ATTEMPT_PATH: &str = "/repos/octo-org/octo-repo/actions/runs/30433642/attempts/2";
const JOBS_PATH: &str = "/repos/octo-org/octo-repo/actions/runs/30433642/attempts/2/jobs";

fn target() -> WatchTarget {
    WatchTarget::GitHub {
        owner: "octo-org".into(),
        repo: "octo-repo".into(),
        workflow: "build.yml".into(),
        branch: Some("main".into()),
    }
}

fn run() -> RunRef {
    RunRef {
        run_id: RunId::new("30433642"),
        attempt: 2,
    }
}

fn adapter(server: &MockServer) -> GitHubActions {
    GitHubActions::new(client(), TestClock::new()).with_base_url(server.uri())
}

fn ok_json(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(fixture(name), "application/json")
}

async fn mount_attempt(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(ATTEMPT_PATH))
        .respond_with(response)
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn attempt_and_paginated_jobs_with_the_failing_step() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ATTEMPT_PATH))
        .and(header("accept", "application/vnd.github+json"))
        .and(header("x-github-api-version", GITHUB_API_VERSION))
        .and(header("user-agent", USER_AGENT))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ok_json("github/run_attempt.json"))
        .expect(1)
        .mount(&server)
        .await;
    let next = format!(
        "<{}{JOBS_PATH}?per_page=100&page=2>; rel=\"next\"",
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path(JOBS_PATH))
        .and(query_param("per_page", "100"))
        .and(query_param_is_missing("page"))
        .respond_with(
            ok_json("github/attempt_jobs_page1.json").insert_header("link", next.as_str()),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(JOBS_PATH))
        .and(query_param("page", "2"))
        .respond_with(ok_json("github/attempt_jobs_page2.json"))
        .expect(1)
        .mount(&server)
        .await;

    let source = adapter(&server);
    assert_eq!(RunDetailsSource::provider(&source), ProviderKind::GitHub);
    let details = source
        .run_details(&target(), &run(), &secret())
        .await
        .unwrap();

    assert_eq!(details.run, run());
    assert_eq!(details.title.as_deref(), Some("Update README.md"));
    assert_eq!(
        details.commit_message.as_deref(),
        Some("Update README.md\n\nExplain the new flag.")
    );
    assert_eq!(details.actor.as_deref(), Some("octocat"));
    assert_eq!(details.triggered_by.as_deref(), Some("hubot"));
    assert_eq!(details.trigger, Trigger::Push);
    assert_eq!(details.run_number, Some(562));
    assert_eq!(
        details.started_at.unwrap().to_rfc3339(),
        "2026-11-16T10:06:00+00:00"
    );
    assert_eq!(
        details.finished_at.unwrap().to_rfc3339(),
        "2026-11-16T10:09:30+00:00"
    );
    assert_eq!(details.jobs_total, 3);
    let jobs = details.jobs.unwrap();
    let summary: Vec<_> = jobs
        .iter()
        .map(|j| (j.name.as_str(), j.status, j.failing_step.as_deref()))
        .collect();
    assert_eq!(
        summary,
        [
            ("lint", BuildStatus::Passed, None),
            (
                "test (ubuntu-latest)",
                BuildStatus::Failed,
                Some("cargo test")
            ),
            ("deploy", BuildStatus::Running, None),
        ]
    );
    assert_eq!(
        jobs[1].url.as_deref(),
        Some("https://github.com/octo-org/octo-repo/actions/runs/30433642/job/399444497")
    );
    assert_eq!(
        jobs[1].finished_at.unwrap().to_rfc3339(),
        "2026-11-16T10:09:00+00:00"
    );
    assert_eq!(jobs[2].finished_at, None);
}

#[tokio::test]
async fn jobs_pagination_is_capped_and_the_total_still_counts_everything() {
    let server = MockServer::start().await;
    mount_attempt(&server, ok_json("github/run_attempt.json")).await;
    // Every page says there is another: only MAX_JOB_PAGES are read.
    let next = format!(
        "<{}{JOBS_PATH}?per_page=100&page=2>; rel=\"next\"",
        server.uri()
    );
    let body = serde_json::json!({
        "total_count": 640,
        "jobs": (0..100).map(|i| serde_json::json!({
            "name": format!("matrix {i}"),
            "status": "completed",
            "conclusion": "success",
        })).collect::<Vec<_>>(),
    });
    Mock::given(method("GET"))
        .and(path(JOBS_PATH))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(body)
                .insert_header("link", next.as_str()),
        )
        .expect(u64::try_from(MAX_JOB_PAGES).unwrap())
        .mount(&server)
        .await;
    let details = adapter(&server)
        .run_details(&target(), &run(), &secret())
        .await
        .unwrap();
    assert_eq!(details.jobs.unwrap().len(), 100 * MAX_JOB_PAGES);
    assert_eq!(details.jobs_total, 640);
}

#[tokio::test]
async fn a_first_attempt_uses_attempt_one_and_has_no_rerun() {
    let server = MockServer::start().await;
    let mut attempt: serde_json::Value =
        serde_json::from_str(&fixture("github/run_attempt.json")).unwrap();
    attempt["run_attempt"] = 1.into();
    attempt["triggering_actor"] = serde_json::json!({"login": "octocat"});
    attempt["event"] = "schedule".into();
    Mock::given(method("GET"))
        .and(path(
            "/repos/octo-org/octo-repo/actions/runs/30433642/attempts/1",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(attempt))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/repos/octo-org/octo-repo/actions/runs/30433642/attempts/1/jobs",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"total_count": 0, "jobs": []})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let first = RunRef {
        run_id: RunId::new("30433642"),
        attempt: 1,
    };
    let details = adapter(&server)
        .run_details(&target(), &first, &secret())
        .await
        .unwrap();
    assert_eq!(details.run.attempt, 1);
    assert_eq!(details.triggered_by, None);
    assert_eq!(details.trigger, Trigger::Schedule);
    assert_eq!(details.jobs, Some(Vec::new()));
    assert_eq!(details.jobs_total, 0);
}

async fn error_for(attempt: ResponseTemplate, jobs: ResponseTemplate) -> ProviderError {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ATTEMPT_PATH))
        .respond_with(attempt)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(JOBS_PATH))
        .respond_with(jobs)
        .mount(&server)
        .await;
    let error = adapter(&server)
        .run_details(&target(), &run(), &secret())
        .await
        .unwrap_err();
    assert_no_token(&error);
    error
}

#[tokio::test]
async fn errors_are_classified_and_never_hold_the_token() {
    let gh = ProviderKind::GitHub;
    let jobs_ok = || ok_json("github/attempt_jobs_page2.json");
    assert_eq!(
        error_for(ResponseTemplate::new(404), jobs_ok()).await,
        ProviderError::NotFound { provider: gh }
    );
    assert_eq!(
        error_for(
            ResponseTemplate::new(403).set_body_raw(
                fixture("github/error_bad_credentials.json"),
                "application/json"
            ),
            jobs_ok()
        )
        .await,
        ProviderError::AccessDenied { provider: gh }
    );
    assert_eq!(
        error_for(
            ok_json("github/run_attempt.json"),
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("retry-after", "42")
        )
        .await,
        ProviderError::RateLimited {
            provider: gh,
            retry_after: Some(std::time::Duration::from_secs(42)),
        }
    );
    assert!(matches!(
        error_for(
            ResponseTemplate::new(200).set_body_raw("{\"jobs\": 3", "application/json"),
            jobs_ok()
        )
        .await,
        ProviderError::Malformed(_)
    ));
    assert!(matches!(
        error_for(
            ok_json("github/run_attempt.json"),
            ResponseTemplate::new(200).set_body_raw("[]", "application/json")
        )
        .await,
        ProviderError::Malformed(_)
    ));
    assert_eq!(
        error_for(ResponseTemplate::new(502), jobs_ok()).await,
        ProviderError::Server {
            provider: gh,
            status: 502
        }
    );
}

#[tokio::test]
async fn a_cloudflare_target_is_not_found_on_github() {
    let server = MockServer::start().await;
    let cf = WatchTarget::Cloudflare {
        account_id: "a".into(),
        worker: "w".into(),
        branch: None,
    };
    assert_eq!(
        adapter(&server)
            .run_details(&cf, &run(), &secret())
            .await
            .unwrap_err(),
        ProviderError::NotFound {
            provider: ProviderKind::GitHub
        }
    );
}
