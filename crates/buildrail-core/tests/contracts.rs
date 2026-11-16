//! Basic guarantees of the shared contracts in `domain` and `ports`: the
//! serialised shapes other crates (profile files, snapshot cache) rely on,
//! status wording, and secret redaction.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use buildrail_core::domain::{
    BuildStatus, ConnectionAlias, Freshness, ProviderKind, RequestKey, RunId, StaleReason, Tone,
    Watch, WatchId, WatchSnapshot, WatchTarget,
};
use buildrail_core::ports::{Clock, CredentialError, ProviderError, Secret, SystemClock};
use serde_json::json;

#[test]
fn watch_target_round_trips_with_provider_tag() {
    let github = WatchTarget::GitHub {
        owner: "acme".into(),
        repo: "app".into(),
        workflow: "ci.yml".into(),
        branch: Some("main".into()),
    };
    let value = serde_json::to_value(&github).unwrap();
    assert_eq!(
        value,
        json!({"provider": "github", "owner": "acme", "repo": "app", "workflow": "ci.yml", "branch": "main"})
    );
    assert_eq!(
        serde_json::from_value::<WatchTarget>(value).unwrap(),
        github
    );

    let cloudflare = WatchTarget::Cloudflare {
        account_id: "abc".into(),
        worker: "site".into(),
        branch: None,
    };
    let value = serde_json::to_value(&cloudflare).unwrap();
    assert_eq!(
        value,
        json!({"provider": "cloudflare", "account_id": "abc", "worker": "site"})
    );
    assert_eq!(
        serde_json::from_value::<WatchTarget>(value).unwrap(),
        cloudflare
    );
    assert_eq!(github.provider(), ProviderKind::GitHub);
    assert_eq!(github.branch(), Some("main"));
    assert_eq!(cloudflare.provider(), ProviderKind::Cloudflare);
    assert_eq!(cloudflare.branch(), None);
}

#[test]
fn watch_and_request_key() {
    let watch = Watch {
        id: WatchId::new("w1"),
        label: "App CI".into(),
        connection: ConnectionAlias::new("work-github"),
        target: WatchTarget::GitHub {
            owner: "acme".into(),
            repo: "app".into(),
            workflow: "ci.yml".into(),
            branch: None,
        },
    };
    let text = serde_json::to_string(&watch).unwrap();
    assert!(text.contains(r#""id":"w1""#), "ids are transparent: {text}");
    assert_eq!(serde_json::from_str::<Watch>(&text).unwrap(), watch);
    let key = RequestKey::from(&watch);
    assert_eq!(key.connection.as_str(), "work-github");
    assert_eq!(key.target, watch.target);
    assert_eq!(watch.id.to_string(), "w1");
    assert_eq!(RunId::new("42").as_str(), "42");
}

#[test]
fn provider_names() {
    assert_eq!(ProviderKind::GitHub.to_string(), "GitHub");
    assert_eq!(ProviderKind::Cloudflare.display_name(), "Cloudflare");
    assert_eq!(
        serde_json::to_string(&ProviderKind::GitHub).unwrap(),
        r#""github""#
    );
    assert_eq!(
        serde_json::to_string(&ProviderKind::Cloudflare).unwrap(),
        r#""cloudflare""#
    );
}

#[test]
fn build_status_tone_activity_and_labels() {
    use BuildStatus::*;
    let table = [
        (Queued, Tone::Amber, true, "Queued", "queued"),
        (Running, Tone::Amber, true, "Building", "running"),
        (Passed, Tone::Green, false, "Passed", "passed"),
        (Deployed, Tone::Green, false, "Deployed", "deployed"),
        (Failed, Tone::Red, false, "Failed", "failed"),
        (Cancelled, Tone::Neutral, false, "Cancelled", "cancelled"),
        (Skipped, Tone::Neutral, false, "Skipped", "skipped"),
        (
            Unavailable,
            Tone::Neutral,
            false,
            "Unavailable",
            "unavailable",
        ),
    ];
    let mut labels = Vec::new();
    for (status, tone, active, label, wire) in table {
        assert_eq!(status.tone(), tone, "{status:?}");
        assert_eq!(status.is_active(), active, "{status:?}");
        assert_eq!(status.label(), label);
        let text = serde_json::to_string(&status).unwrap();
        assert_eq!(text, format!("\"{wire}\""));
        assert_eq!(serde_json::from_str::<BuildStatus>(&text).unwrap(), status);
        labels.push(label);
    }
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(
        labels.len(),
        table.len(),
        "labels distinguishable without colour"
    );
}

#[test]
fn snapshot_round_trip_and_pending() {
    let snap = WatchSnapshot {
        freshness: Freshness::Stale {
            reason: StaleReason::RateLimited,
        },
        ..WatchSnapshot::pending(WatchId::new("w"))
    };
    let value = serde_json::to_value(&snap).unwrap();
    assert_eq!(
        value["freshness"],
        json!({"kind": "stale", "reason": "rate_limited"})
    );
    assert_eq!(
        serde_json::from_value::<WatchSnapshot>(value).unwrap(),
        snap
    );
    let pending = WatchSnapshot::pending(WatchId::new("p"));
    assert_eq!(pending.freshness, Freshness::Pending);
    assert_eq!(pending.record, None);
    assert_eq!(pending.last_success, None);
}

#[test]
fn secret_is_never_printed() {
    let secret = Secret::new("ghp_supersecret");
    assert_eq!(format!("{secret:?}"), "Secret(***)");
    assert_eq!(secret.to_string(), "***");
    assert_eq!(secret.expose(), "ghp_supersecret");
    assert_eq!(secret, Secret::new("ghp_supersecret"));
}

#[test]
fn error_messages() {
    let p = ProviderKind::GitHub;
    assert_eq!(
        ProviderError::AccessDenied { provider: p }.to_string(),
        "access denied by GitHub"
    );
    assert_eq!(
        ProviderError::NotFound { provider: p }.to_string(),
        "target not found on GitHub"
    );
    assert_eq!(
        ProviderError::RateLimited {
            provider: p,
            retry_after: Some(Duration::from_secs(1))
        }
        .to_string(),
        "rate limited by GitHub"
    );
    assert_eq!(
        ProviderError::Network("timeout".into()).to_string(),
        "network error: timeout"
    );
    assert_eq!(
        ProviderError::Server {
            provider: p,
            status: 503
        }
        .to_string(),
        "GitHub returned HTTP 503"
    );
    assert_eq!(
        ProviderError::Malformed("eof".into()).to_string(),
        "unreadable response: eof"
    );
    assert_eq!(
        CredentialError("locked".into()).to_string(),
        "credential store error: locked"
    );
}

#[test]
fn system_clock_is_close_to_now() {
    let delta = chrono::Utc::now() - SystemClock.now();
    assert!(delta.num_seconds().abs() < 5);
}
