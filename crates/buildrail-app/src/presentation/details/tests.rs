#![allow(clippy::unwrap_used)]

use buildrail_core::details::{JobSummary, RunRef};
use buildrail_core::domain::{ConnectionAlias, RunId};
use buildrail_core::ports::ProviderError;
use chrono::TimeZone;

use super::*;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 16, 10, 20, 0).unwrap()
}

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

fn gh(branch: Option<&str>) -> Watch {
    Watch {
        id: WatchId::new("keevoro"),
        label: "Keevoro".into(),
        connection: ConnectionAlias::new("work"),
        target: WatchTarget::GitHub {
            owner: "acme".into(),
            repo: "keevoro".into(),
            workflow: "ci.yml".into(),
            branch: branch.map(str::to_owned),
        },
    }
}

fn cf() -> Watch {
    Watch {
        id: WatchId::new("site"),
        label: "Site".into(),
        connection: ConnectionAlias::new("cf"),
        target: WatchTarget::Cloudflare {
            account_id: "0123".into(),
            worker: "site".into(),
            branch: None,
        },
    }
}

fn record(
    run: &str,
    status: BuildStatus,
    created_min_ago: i64,
    updated_min_ago: i64,
) -> BuildRecord {
    BuildRecord {
        run_id: RunId::new(run),
        attempt: 1,
        status,
        url: format!("https://github.com/acme/keevoro/actions/runs/{run}"),
        branch: Some("main".into()),
        commit: Some("a7c412e9f0b1c2d3".into()),
        failing_job: None,
        created_at: now() - TimeDelta::minutes(created_min_ago),
        updated_at: now() - TimeDelta::minutes(updated_min_ago),
        observed_at: now(),
    }
}

fn fresh(watch: &Watch, record: Option<BuildRecord>) -> WatchSnapshot {
    WatchSnapshot {
        watch_id: watch.id.clone(),
        record,
        freshness: Freshness::Fresh,
        last_success: Some(now()),
    }
}

fn job(
    name: &str,
    status: BuildStatus,
    secs: Option<(i64, i64)>,
    step: Option<&str>,
) -> JobSummary {
    JobSummary {
        name: name.into(),
        status,
        started_at: secs.map(|(start, _)| now() - TimeDelta::seconds(start)),
        finished_at: secs
            .filter(|(_, end)| *end >= 0)
            .map(|(_, end)| now() - TimeDelta::seconds(end)),
        url: Some(format!(
            "https://github.com/acme/keevoro/actions/runs/7/job/{name}"
        )),
        failing_step: step.map(str::to_owned),
    }
}

fn details(run: &str, attempt: u32) -> RunDetails {
    let mut d = RunDetails::new(RunRef {
        run_id: RunId::new(run),
        attempt,
    });
    d.title = Some("Fix the flaky login test".into());
    d.commit_message = Some("Fix the flaky login test\n\nLonger story.".into());
    d.actor = Some("octocat".into());
    d.trigger = Trigger::Push;
    d.run_number = Some(562);
    d.started_at = Some(now() - TimeDelta::seconds(12 * 60));
    d.finished_at = Some(now() - TimeDelta::seconds(12 * 60 - 252));
    d
}

fn build(
    watch: &Watch,
    snapshot: &WatchSnapshot,
    details: Option<&RunDetails>,
    recent: Option<&[BuildRecord]>,
    load: &DetailsLoad,
) -> DetailsModel {
    DetailsModel::build(DetailsInput {
        watch,
        snapshot,
        details,
        recent,
        load,
        now: now(),
        offset: utc(),
    })
}

fn fact<'a>(model: &'a DetailsModel, label: &str) -> Option<&'a Fact> {
    model.run.iter().find(|f| f.label == label)
}

fn value(model: &DetailsModel, label: &str) -> Option<String> {
    fact(model, label).map(|f| f.value.clone())
}

#[test]
fn durations_are_said_the_way_people_say_them() {
    let cases = [
        (-5, "0 s"),
        (0, "0 s"),
        (45, "45 s"),
        (60, "1 min"),
        (252, "4 min 12 s"),
        (3_599, "59 min 59 s"),
        (3_600, "1 h"),
        (3_900, "1 h 5 min"),
        (90_000, "25 h"),
    ];
    for (secs, expected) in cases {
        assert_eq!(duration_words(TimeDelta::seconds(secs)), expected, "{secs}");
    }
    let running = [
        (0, "under a minute"),
        (59, "under a minute"),
        (60, "1 min"),
        (179, "2 min"),
        (3_659, "1 h"),
        (3_719, "1 h 1 min"),
        (7_200, "2 h"),
    ];
    for (secs, expected) in running {
        assert_eq!(running_words(TimeDelta::seconds(secs)), expected, "{secs}");
    }
}

#[test]
fn triggers_name_who_or_what_started_the_run() {
    let base = details("7", 1);
    let with = |trigger: Trigger, actor: Option<&str>| {
        let mut d = base.clone();
        d.trigger = trigger;
        d.actor = actor.map(str::to_owned);
        trigger_words(&d)
    };
    let cases = [
        (Trigger::Push, Some("octocat"), Some("pushed by octocat")),
        (Trigger::Push, None, Some("pushed")),
        (
            Trigger::PullRequest,
            Some("mona"),
            Some("pull request by mona"),
        ),
        (Trigger::Schedule, Some("github"), Some("scheduled")),
        (Trigger::Manual, Some("mona"), Some("started by mona")),
        (Trigger::Manual, None, Some("started by hand")),
        (Trigger::Api, None, Some("started through the API")),
        (Trigger::Workflow, None, Some("after another workflow")),
        (
            Trigger::Other("merge_group".into()),
            Some("mona"),
            Some("merge group by mona"),
        ),
        (Trigger::Unknown, Some("mona"), Some("started by mona")),
        (Trigger::Unknown, None, None),
    ];
    for (trigger, actor, expected) in cases {
        assert_eq!(
            with(trigger.clone(), actor).as_deref(),
            expected,
            "{trigger:?} {actor:?}"
        );
    }
    let mut rerun = details("7", 2);
    rerun.triggered_by = Some("hubot".into());
    assert_eq!(trigger_words(&rerun).as_deref(), Some("re-run by hubot"));
    rerun.triggered_by = None;
    assert_eq!(trigger_words(&rerun).as_deref(), Some("re-run by octocat"));
    rerun.actor = None;
    assert_eq!(trigger_words(&rerun).as_deref(), Some("re-run"));
}

#[test]
fn first_lines_are_trimmed_and_cut() {
    assert_eq!(first_line("\n  Fix it  \nmore"), "Fix it");
    assert_eq!(first_line(""), "");
    let long = "x".repeat(100);
    let cut = first_line(&long);
    assert_eq!(cut.chars().count(), MAX_MESSAGE);
    assert!(cut.ends_with('…'));
    assert_eq!(exact_time(now(), utc()), "16 Nov 2026, 10:20");
    let plus_one = FixedOffset::east_opt(3_600).unwrap();
    assert_eq!(exact_time(now(), plus_one), "16 Nov 2026, 11:20");
}

#[test]
fn a_failed_run_with_details_jobs_failed_first_and_history() {
    let watch = gh(Some("main"));
    let current = record("7", BuildStatus::Failed, 13, 8);
    let snapshot = fresh(&watch, Some(current.clone()));
    let mut d = details("7", 1);
    d.jobs = Some(vec![
        job("lint", BuildStatus::Passed, Some((700, 640)), None),
        job(
            "test (ubuntu-latest)",
            BuildStatus::Failed,
            Some((700, 525)),
            Some("cargo test"),
        ),
        job("build", BuildStatus::Skipped, None, None),
        job(
            "test (windows-latest)",
            BuildStatus::Failed,
            Some((700, 500)),
            None,
        ),
    ]);
    d.jobs_total = 4;
    let history = vec![
        current.clone(),
        record("6", BuildStatus::Passed, 60, 55),
        record("5", BuildStatus::Failed, 120, 115),
        record("4", BuildStatus::Passed, 180, 175),
    ];
    let model = build(
        &watch,
        &snapshot,
        Some(&d),
        Some(&history),
        &DetailsLoad::Loaded,
    );

    assert_eq!(model.label, "Keevoro");
    assert_eq!(model.status_text, "Failed");
    assert_eq!(model.tone, Tone::Red);
    assert_eq!(model.icon, IconKind::Cross);
    assert_eq!(model.facts, "GitHub · acme/keevoro · ci.yml");
    assert_eq!(model.title, "Keevoro details");
    assert_eq!(
        model.accessible_heading(),
        "Keevoro, Failed, GitHub · acme/keevoro · ci.yml"
    );
    assert_eq!(model.notice, None);
    assert_eq!(model.freshness, None);
    assert_eq!(model.load_line, None);

    let rows: Vec<(&str, String)> = model
        .run
        .iter()
        .map(|f| (f.label, f.value.clone()))
        .collect();
    assert_eq!(
        rows,
        [
            ("Branch", "main".to_owned()),
            ("Commit", "a7c412e · Fix the flaky login test".to_owned()),
            ("Trigger", "Pushed by octocat".to_owned()),
            ("Run", "#562".to_owned()),
            ("Started", "12 min ago".to_owned()),
            ("Duration", "Took 4 min 12 s".to_owned()),
        ]
    );
    let started = fact(&model, "Started").unwrap();
    assert_eq!(started.description.as_deref(), Some("16 Nov 2026, 10:08"));
    assert_eq!(
        started.accessible_name(),
        "Started: 12 min ago, 16 Nov 2026, 10:08"
    );
    assert_eq!(
        fact(&model, "Branch").unwrap().accessible_name(),
        "Branch: main"
    );

    let jobs = model.jobs.as_ref().unwrap();
    assert_eq!(jobs.heading, "Jobs");
    let names: Vec<&str> = jobs.rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "test (ubuntu-latest)",
            "test (windows-latest)",
            "lint",
            "build"
        ],
        "failed first, then the provider's order"
    );
    let failed = &jobs.rows[0];
    assert_eq!(failed.duration.as_deref(), Some("2 min 55 s"));
    assert_eq!(
        failed.failing_step.as_deref(),
        Some("Failed at “cargo test”.")
    );
    assert_eq!(
        failed.accessible_name,
        "test (ubuntu-latest), Failed, 2 min 55 s, failed at step cargo test, opens the job"
    );
    assert_eq!(jobs.rows[1].failing_step, None);
    assert_eq!(jobs.rows[3].duration, None, "a skipped job has no time");
    assert_eq!(jobs.more, None);
    assert_eq!(jobs.empty, None);

    let recent = model.recent.as_ref().unwrap();
    assert_eq!(recent.heading, "Recent runs");
    let marks: Vec<(&str, &str, bool)> = recent
        .runs
        .iter()
        .map(|r| (r.status_text.as_str(), r.when.as_str(), r.current))
        .collect();
    assert_eq!(
        marks,
        [
            ("Failed", "13 min ago", true),
            ("Passed", "1 h ago", false),
            ("Failed", "2 h ago", false),
            ("Passed", "3 h ago", false),
        ]
    );
    assert!(
        recent.runs.iter().all(|r| r.branch.is_none()),
        "the watch has a branch"
    );
    assert_eq!(
        recent.runs[0].accessible_name,
        "Failed, 13 minutes ago, the run shown above, opens the run"
    );
    assert_eq!(
        recent.summary.as_deref(),
        Some("2 of the last 4 runs failed.")
    );

    assert_eq!(model.primary.label, "Open run");
    assert_eq!(model.primary.action, PanelAction::Open(current.url.clone()));
    let quiet: Vec<&str> = model.secondary.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(quiet, ["Refresh now", "Copy link", "Edit in Settings"]);
    assert_eq!(
        model.secondary[1].action,
        PanelAction::CopyLink(current.url)
    );
    assert!(model.secondary.iter().all(|b| b.enabled));

    // Next change: "12 min ago" (started 12 min ago, on the minute) at
    // 10:21; the recent runs change on their minutes or hours too.
    assert_eq!(
        model.next_text_change(now()),
        Some(now() + TimeDelta::seconds(60))
    );
}

#[test]
fn jobs_are_capped_with_a_link_to_the_run() {
    let watch = gh(None);
    let snapshot = fresh(&watch, Some(record("7", BuildStatus::Passed, 5, 1)));
    let mut d = details("7", 1);
    d.jobs = Some(
        (0..15)
            .map(|i| {
                job(
                    &format!("matrix {i}"),
                    BuildStatus::Passed,
                    Some((300, 200)),
                    None,
                )
            })
            .collect(),
    );
    d.jobs_total = 17;
    let model = build(&watch, &snapshot, Some(&d), None, &DetailsLoad::Loaded);
    let jobs = model.jobs.unwrap();
    assert_eq!(jobs.rows.len(), MAX_JOBS);
    let more = jobs.more.unwrap();
    assert_eq!(more.text, "and 5 more");
    assert_eq!(more.accessible_name, "and 5 more jobs, opens the run");
    assert_eq!(more.url, "https://github.com/acme/keevoro/actions/runs/7");
    assert_eq!(model.recent, None, "not fetched yet");

    let mut one_more = d.clone();
    one_more.jobs_total = 13;
    one_more.jobs = Some(one_more.jobs.unwrap().into_iter().take(13).collect());
    let model = build(
        &watch,
        &snapshot,
        Some(&one_more),
        None,
        &DetailsLoad::Loaded,
    );
    assert_eq!(
        model.jobs.unwrap().more.unwrap().accessible_name,
        "and 1 more job, opens the run"
    );

    let mut none = d;
    none.jobs = Some(Vec::new());
    none.jobs_total = 0;
    let model = build(&watch, &snapshot, Some(&none), None, &DetailsLoad::Loaded);
    let jobs = model.jobs.unwrap();
    assert!(jobs.rows.is_empty());
    assert_eq!(jobs.empty.as_deref(), Some("No jobs have started yet."));
}

#[test]
fn a_running_run_counts_minutes_and_changes_once_a_minute() {
    let watch = gh(None);
    let mut current = record("8", BuildStatus::Running, 4, 0);
    current.attempt = 2;
    let snapshot = fresh(&watch, Some(current.clone()));
    let mut d = details("8", 2);
    d.triggered_by = Some("hubot".into());
    d.started_at = Some(now() - TimeDelta::seconds(190));
    d.finished_at = None;
    d.jobs = Some(vec![
        job("lint", BuildStatus::Passed, Some((185, 150)), None),
        job("test", BuildStatus::Running, Some((150, -1)), None),
        job("deploy", BuildStatus::Queued, None, None),
    ]);
    d.jobs_total = 3;
    let other_branch = BuildRecord {
        branch: Some("feature/login".into()),
        ..record("5", BuildStatus::Passed, 50, 45)
    };
    let history = vec![current.clone(), other_branch];
    let model = build(
        &watch,
        &snapshot,
        Some(&d),
        Some(&history),
        &DetailsLoad::Loaded,
    );
    assert_eq!(model.status_text, "Building");
    assert_eq!(value(&model, "Trigger").as_deref(), Some("Re-run by hubot"));
    assert_eq!(value(&model, "Run").as_deref(), Some("#562, attempt 2"));
    assert_eq!(value(&model, "Started").as_deref(), Some("3 min ago"));
    assert_eq!(
        value(&model, "Duration").as_deref(),
        Some("Running for 3 min")
    );
    let jobs = model.jobs.as_ref().unwrap();
    let durations: Vec<Option<&str>> = jobs.rows.iter().map(|r| r.duration.as_deref()).collect();
    assert_eq!(
        durations,
        [Some("35 s"), Some("running for 2 min"), Some("waiting")]
    );
    // Branches show when the watch follows every branch.
    let recent = model.recent.as_ref().unwrap();
    assert_eq!(recent.runs[1].branch.as_deref(), Some("feature/login"));
    assert_eq!(
        recent.runs[0].accessible_name,
        "Building, 4 minutes ago, branch main, the run shown above, opens the run"
    );
    assert_eq!(
        recent.summary.as_deref(),
        Some("None of the last 2 runs failed.")
    );
    // The running time (190 s) turns "3 min" into "4 min" in 50 s; the
    // job's (150 s) changes in 30 s, which is the earliest.
    assert_eq!(
        model.next_text_change(now()),
        Some(now() + TimeDelta::seconds(30))
    );
}

#[test]
fn partial_facts_while_loading_then_a_failure_keeps_them() {
    let watch = gh(Some("main"));
    let current = record("7", BuildStatus::Passed, 10, 6);
    let snapshot = fresh(&watch, Some(current));
    let loading = build(&watch, &snapshot, None, None, &DetailsLoad::Loading);
    assert_eq!(
        loading.load_line,
        Some(LoadLine {
            text: "Loading details…".into(),
            failed: false
        })
    );
    // The snapshot's own facts, at once.
    assert_eq!(value(&loading, "Branch").as_deref(), Some("main"));
    assert_eq!(value(&loading, "Commit").as_deref(), Some("a7c412e"));
    assert_eq!(value(&loading, "Started").as_deref(), Some("10 min ago"));
    assert_eq!(value(&loading, "Duration").as_deref(), Some("Took 4 min"));
    assert_eq!(value(&loading, "Trigger"), None);
    assert_eq!(loading.jobs, None);
    let refresh = &loading.secondary[0];
    assert_eq!(refresh.label, "Refreshing…");
    assert!(!refresh.enabled);

    let failures = [
        (
            DetailsError::Provider(ProviderKind::GitHub, ProviderError::Network("dns".into())),
            "Details could not be loaded: GitHub could not be reached.",
        ),
        (
            DetailsError::Provider(
                ProviderKind::GitHub,
                ProviderError::AccessDenied {
                    provider: ProviderKind::GitHub,
                },
            ),
            "Details could not be loaded: GitHub refused access.",
        ),
        (
            DetailsError::Provider(
                ProviderKind::GitHub,
                ProviderError::NotFound {
                    provider: ProviderKind::GitHub,
                },
            ),
            "Details could not be loaded: GitHub no longer has this run.",
        ),
        (
            DetailsError::Provider(
                ProviderKind::GitHub,
                ProviderError::RateLimited {
                    provider: ProviderKind::GitHub,
                    retry_after: None,
                },
            ),
            "Details could not be loaded: GitHub is limiting requests for now.",
        ),
        (
            DetailsError::Provider(
                ProviderKind::Cloudflare,
                ProviderError::Server {
                    provider: ProviderKind::Cloudflare,
                    status: 502,
                },
            ),
            "Details could not be loaded: Cloudflare had a problem (HTTP 502).",
        ),
        (
            DetailsError::Provider(ProviderKind::GitHub, ProviderError::Malformed("x".into())),
            "Details could not be loaded: GitHub sent something BuildRail could not read.",
        ),
        (
            DetailsError::NoCredential(ConnectionAlias::new("work")),
            "Details could not be loaded: there is no token for the “work” connection on this device.",
        ),
        (
            DetailsError::CredentialStore,
            "Details could not be loaded: the system's credential store did not answer.",
        ),
        (
            DetailsError::NoSource(ProviderKind::Cloudflare),
            "Details could not be loaded: Cloudflare details are not available in this build.",
        ),
    ];
    for (error, text) in failures {
        let load = DetailsLoad::Failed(error);
        let model = build(&watch, &snapshot, None, None, &load);
        let line = model.load_line.clone().unwrap();
        assert_eq!(line.text, text);
        assert!(line.failed);
        assert_eq!(
            value(&model, "Branch").as_deref(),
            Some("main"),
            "basic facts stay"
        );
        assert_eq!(model.primary.label, "Open run");
        assert_eq!(model.secondary[0].label, "Refresh now");
    }

    // Details for another run (the strip moved on) are not shown.
    let old = details("6", 1);
    let model = build(&watch, &snapshot, Some(&old), None, &DetailsLoad::Loading);
    assert_eq!(value(&model, "Trigger"), None);
    assert!(model.load_line.is_some());
}

#[test]
fn a_queued_run_waits_and_a_message_without_a_commit_still_shows() {
    let watch = gh(Some("main"));
    let mut queued = record("9", BuildStatus::Queued, 1, 1);
    queued.commit = None;
    let snapshot = fresh(&watch, Some(queued));
    let model = build(&watch, &snapshot, None, None, &DetailsLoad::Loading);
    assert_eq!(value(&model, "Started"), None);
    assert_eq!(
        value(&model, "Duration").as_deref(),
        Some("Waiting to start")
    );
    assert_eq!(value(&model, "Commit"), None);
    let mut d = details("9", 1);
    d.started_at = None;
    d.finished_at = None;
    d.run_number = None;
    let model = build(&watch, &snapshot, Some(&d), None, &DetailsLoad::Loaded);
    assert_eq!(
        value(&model, "Commit").as_deref(),
        Some("Fix the flaky login test")
    );
    assert_eq!(value(&model, "Run"), None);
    assert_eq!(model.icon, IconKind::Queued);
    // Nothing on screen measures time from anything but the strip's age.
    assert_eq!(model.next_text_change(now()), None);
}

#[test]
fn reconnect_explains_and_offers_reconnect() {
    let watch = gh(Some("main"));
    let mut snapshot = fresh(&watch, Some(record("7", BuildStatus::Failed, 30, 25)));
    snapshot.freshness = Freshness::Stale {
        reason: StaleReason::AccessDenied,
    };
    snapshot.last_success = Some(now() - TimeDelta::minutes(5));
    let model = build(&watch, &snapshot, None, None, &DetailsLoad::Idle);
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.problem);
    assert_eq!(
        notice.text,
        "GitHub no longer accepts the token for this build, so BuildRail cannot check it. Reconnect to give it a new one."
    );
    assert_eq!(model.primary.label, "Reconnect");
    assert_eq!(model.primary.action, PanelAction::Reconnect);
    let quiet: Vec<&str> = model.secondary.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(quiet, ["Open run", "Edit in Settings"]);
    assert_eq!(
        model.load_line, None,
        "nothing is fetched while it needs reconnecting"
    );
    assert_eq!(
        model.freshness.as_deref(),
        Some("Last checked 5 min ago · reconnect needed.")
    );
    // Without a cached run.
    snapshot.record = None;
    let model = build(&watch, &snapshot, None, None, &DetailsLoad::Idle);
    assert_eq!(model.status_text, "Reconnect needed");
    assert_eq!(model.primary.label, "Reconnect");
    let quiet: Vec<&str> = model.secondary.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(quiet, ["Edit in Settings"]);
    assert!(model.run.is_empty());
}

#[test]
fn stale_and_cached_runs_say_when_they_were_last_checked() {
    let watch = gh(Some("main"));
    let mut snapshot = fresh(&watch, Some(record("7", BuildStatus::Passed, 30, 25)));
    snapshot.last_success = Some(now() - TimeDelta::seconds(5 * 60 + 10));
    for (reason, word) in [
        (StaleReason::Offline, "offline"),
        (StaleReason::RateLimited, "rate limited"),
        (StaleReason::NotFound, "not found"),
        (StaleReason::ProviderError, "provider error"),
    ] {
        snapshot.freshness = Freshness::Stale { reason };
        let model = build(&watch, &snapshot, None, None, &DetailsLoad::Loading);
        assert_eq!(
            model.freshness,
            Some(format!("Last checked 5 min ago · {word}."))
        );
        assert_eq!(
            model.notice, None,
            "a run is shown, nothing to explain first"
        );
    }
    snapshot.freshness = Freshness::Pending;
    let model = build(&watch, &snapshot, None, None, &DetailsLoad::Loading);
    assert_eq!(
        model.freshness.as_deref(),
        Some("Last checked 5 min ago · checking again now.")
    );
    // The freshness age is one of the anchors: 5 min 10 s → 6 min at 50 s.
    assert_eq!(
        model.next_text_change(now()),
        Some(now() + TimeDelta::seconds(50))
    );
    snapshot.last_success = None;
    let mut seen = snapshot.record.clone().unwrap();
    seen.observed_at = now() - TimeDelta::minutes(2);
    snapshot.record = Some(seen);
    let model = build(&watch, &snapshot, None, None, &DetailsLoad::Loading);
    assert_eq!(
        model.freshness.as_deref(),
        Some("Last checked 2 min ago · checking again now.")
    );
}

#[test]
fn no_runs_pending_and_unreachable_builds_explain_themselves() {
    let watch = gh(Some("main"));
    let empty = fresh(&watch, None);
    let model = build(&watch, &empty, None, None, &DetailsLoad::Idle);
    let notice = model.notice.as_ref().unwrap();
    assert_eq!(notice.heading.as_deref(), Some("No runs yet."));
    assert_eq!(
        notice.text,
        "ci.yml has not run on branch main yet. Its first run will show here as soon as it starts."
    );
    assert!(!notice.problem);
    assert_eq!(model.status_text, "No runs yet");
    assert_eq!(model.primary.label, "Refresh now");
    assert_eq!(model.primary.action, PanelAction::Refresh);
    assert!(model.primary.enabled);
    assert_eq!(model.secondary.len(), 1);
    assert_eq!(model.load_line, None);
    assert!(model.run.is_empty());
    assert_eq!(model.next_text_change(now()), None);

    let site = cf();
    let model = build(&site, &fresh(&site, None), None, None, &DetailsLoad::Idle);
    assert_eq!(
        model.notice.unwrap().text,
        "site has not been built yet. Its first build will show here as soon as it starts."
    );
    assert_eq!(model.facts, "Cloudflare · Worker site");

    let pending = WatchSnapshot::pending(watch.id.clone());
    let model = build(&watch, &pending, None, None, &DetailsLoad::Idle);
    assert_eq!(model.status_text, "Checking…");
    assert_eq!(
        model.notice.unwrap().text,
        "BuildRail is checking this build for the first time."
    );

    for (reason, text) in [
        (
            StaleReason::NotFound,
            "GitHub cannot find this build, or the token cannot see it. Check it in Settings.",
        ),
        (
            StaleReason::Offline,
            "BuildRail cannot reach GitHub right now. It tries again on its own.",
        ),
        (
            StaleReason::RateLimited,
            "GitHub is limiting requests for now. BuildRail tries again shortly.",
        ),
        (
            StaleReason::ProviderError,
            "GitHub had a problem answering. BuildRail tries again shortly.",
        ),
    ] {
        let snapshot = WatchSnapshot {
            freshness: Freshness::Stale { reason },
            ..empty.clone()
        };
        let model = build(&watch, &snapshot, None, None, &DetailsLoad::Idle);
        let notice = model.notice.unwrap();
        assert_eq!(notice.text, text);
        assert!(notice.problem);
        assert_eq!(model.icon, IconKind::Warning);
        assert_eq!(model.freshness, None);
    }
}

#[test]
fn cloudflare_has_no_jobs_and_says_builds() {
    let site = cf();
    let mut current = record("u-3", BuildStatus::Deployed, 20, 18);
    current.url = "https://dash.cloudflare.com/x/builds/u-3".into();
    let snapshot = fresh(&site, Some(current.clone()));
    let mut d = RunDetails::new(RunRef::from(&current));
    d.commit_message = Some("Add new feature".into());
    d.actor = Some("dev@example.com".into());
    d.trigger = Trigger::Push;
    d.started_at = Some(now() - TimeDelta::seconds(20 * 60 - 5));
    d.finished_at = Some(now() - TimeDelta::seconds(18 * 60));
    let history = vec![
        current.clone(),
        record("u-2", BuildStatus::Failed, 60, 59),
        record("u-1", BuildStatus::Failed, 70, 69),
    ];
    let model = build(
        &site,
        &snapshot,
        Some(&d),
        Some(&history),
        &DetailsLoad::Loaded,
    );
    assert_eq!(model.jobs, None);
    assert_eq!(
        value(&model, "Trigger").as_deref(),
        Some("Pushed by dev@example.com")
    );
    assert_eq!(
        value(&model, "Duration").as_deref(),
        Some("Took 1 min 55 s")
    );
    let recent = model.recent.unwrap();
    assert_eq!(recent.heading, "Recent builds");
    assert_eq!(
        recent.summary.as_deref(),
        Some("2 of the last 3 builds failed.")
    );
    assert!(recent.runs[0]
        .accessible_name
        .ends_with("the build shown above, opens the build"));
    assert_eq!(
        recent.runs[1].branch.as_deref(),
        Some("main"),
        "every branch is followed"
    );
}

#[test]
fn recent_runs_collapse_attempts_cap_and_skip_a_lone_run() {
    let watch = gh(Some("main"));
    let current = record("20", BuildStatus::Failed, 1, 1);
    let snapshot = fresh(&watch, Some(current.clone()));
    // Only the current run: nothing to compare with.
    let model = build(
        &watch,
        &snapshot,
        None,
        Some(std::slice::from_ref(&current)),
        &DetailsLoad::Loaded,
    );
    assert_eq!(model.recent, None);
    // Ten earlier runs, one listed twice (two attempts): capped at eight,
    // one mark per run, and the strip's own record wins for the current.
    let mut history: Vec<BuildRecord> = (10..20)
        .map(|i| record(&i.to_string(), BuildStatus::Failed, 100 - i, 100 - i))
        .collect();
    let mut second = record("19", BuildStatus::Passed, 81, 80);
    second.attempt = 2;
    history.push(second);
    let mut older_view = current.clone();
    older_view.status = BuildStatus::Running;
    history.push(older_view);
    let model = build(
        &watch,
        &snapshot,
        None,
        Some(&history),
        &DetailsLoad::Loaded,
    );
    let recent = model.recent.unwrap();
    assert_eq!(recent.runs.len(), MAX_RECENT);
    assert_eq!(recent.runs[0].status_text, "Failed", "the strip's record");
    assert!(recent.runs[0].current);
    assert_eq!(
        recent.runs[1].status_text, "Passed",
        "run 19's newest attempt"
    );
    assert_eq!(
        recent.summary.as_deref(),
        Some("7 of the last 8 runs failed.")
    );
    let all_failed: Vec<BuildRecord> = (10..13)
        .map(|i| record(&i.to_string(), BuildStatus::Failed, 100 - i, 100 - i))
        .collect();
    let snapshot = fresh(&watch, Some(all_failed[2].clone()));
    let model = build(
        &watch,
        &snapshot,
        None,
        Some(&all_failed),
        &DetailsLoad::Loaded,
    );
    assert_eq!(
        model.recent.unwrap().summary.as_deref(),
        Some("All of the last 3 runs failed.")
    );
    // No run on the strip: the history is not shown on its own.
    let empty = fresh(&watch, None);
    let model = build(
        &watch,
        &empty,
        None,
        Some(&all_failed),
        &DetailsLoad::Loaded,
    );
    assert_eq!(model.recent, None);
}
