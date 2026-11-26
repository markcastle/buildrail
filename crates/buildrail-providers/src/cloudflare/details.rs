//! [`CloudflareBuilds`] as a [`RunDetailsSource`]: one build's commit
//! message, author, trigger and timings.
//!
//! Workers Builds' list endpoint already carries everything the panel
//! can show (`build_trigger_metadata.commit_message`, `author`,
//! `build_trigger_source`, and `initializing_on` / `running_on` /
//! `stopped_on`), so details come from the same
//! `GET /accounts/{account_id}/builds/workers/{script_tag}/builds` list:
//! a page listed in the last few seconds (the panel asks for the history
//! first) is reused, otherwise one page of
//! [`FILTERED_PAGE_SIZE`](super::runs::FILTERED_PAGE_SIZE) builds is listed
//! and the build is looked up by its UUID. The API documents no stages or
//! steps for a build (only its log), so `jobs` is `None`.

use async_trait::async_trait;
use buildrail_core::details::{RunDetails, RunRef};
use buildrail_core::domain::{ProviderKind, WatchTarget};
use buildrail_core::ports::{ProviderError, RunDetailsSource, Secret};

use super::runs::{CloudflareBuilds, WireBuild, FILTERED_PAGE_SIZE};
use super::status::{build_started_at, map_trigger_source};

#[async_trait]
impl RunDetailsSource for CloudflareBuilds {
    fn provider(&self) -> ProviderKind {
        ProviderKind::Cloudflare
    }

    async fn run_details(
        &self,
        target: &WatchTarget,
        run: &RunRef,
        credential: &Secret,
    ) -> Result<RunDetails, ProviderError> {
        let WatchTarget::Cloudflare {
            account_id, worker, ..
        } = target
        else {
            return Err(ProviderError::NotFound {
                provider: ProviderKind::Cloudflare,
            });
        };
        let find = |builds: Vec<WireBuild>| {
            builds
                .into_iter()
                .find(|build| build.build_uuid == run.run_id.as_str())
        };
        let reused = self
            .recent_list(account_id, worker, credential)
            .and_then(find);
        let build = match reused {
            Some(build) => build,
            None => find(
                self.fetch_builds(account_id, worker, FILTERED_PAGE_SIZE, credential)
                    .await?,
            )
            .ok_or(ProviderError::NotFound {
                provider: ProviderKind::Cloudflare,
            })?,
        };
        Ok(into_details(build, run))
    }
}

/// The details of one listed build.
fn into_details(build: WireBuild, run: &RunRef) -> RunDetails {
    let mut details = RunDetails::new(RunRef {
        run_id: run.run_id.clone(),
        attempt: 1,
    });
    details.started_at = build_started_at(
        &build.status,
        build.initializing_on,
        build.running_on,
        build.created_on,
    );
    details.finished_at = (build.status == "stopped")
        .then(|| build.stopped_on.or(build.modified_on))
        .flatten();
    if let Some(meta) = build.build_trigger_metadata {
        details.trigger = map_trigger_source(meta.build_trigger_source.as_deref());
        details.commit_message = meta.commit_message.filter(|m| !m.trim().is_empty());
        details.actor = meta.author.filter(|a| !a.trim().is_empty());
    }
    details
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use buildrail_core::details::Trigger;
    use buildrail_core::domain::RunId;

    use super::*;

    fn build(json: &str) -> WireBuild {
        serde_json::from_str(json).unwrap()
    }

    fn run() -> RunRef {
        RunRef {
            run_id: RunId::new("u"),
            attempt: 1,
        }
    }

    #[test]
    fn stopped_build_with_metadata() {
        let details = into_details(
            build(
                r#"{"build_uuid":"u","status":"stopped","build_outcome":"success","created_on":"2026-11-16T10:00:00Z","initializing_on":"2026-11-16T10:00:01Z","running_on":"2026-11-16T10:00:05Z","stopped_on":"2026-11-16T10:02:00Z","build_trigger_metadata":{"build_trigger_source":"push","commit_message":"Add new feature","author":"dev@example.com"}}"#,
            ),
            &run(),
        );
        assert_eq!(
            details.started_at.unwrap().to_rfc3339(),
            "2026-11-16T10:00:05+00:00"
        );
        assert_eq!(
            details.finished_at.unwrap().to_rfc3339(),
            "2026-11-16T10:02:00+00:00"
        );
        assert_eq!(details.trigger, Trigger::Push);
        assert_eq!(details.commit_message.as_deref(), Some("Add new feature"));
        assert_eq!(details.actor.as_deref(), Some("dev@example.com"));
        assert_eq!(details.jobs, None, "Workers Builds has no jobs");
        assert_eq!(details.run_number, None);
    }

    #[test]
    fn queued_and_bare_builds() {
        let queued = into_details(
            build(
                r#"{"build_uuid":"u","status":"queued","created_on":"2026-11-16T10:00:00Z","build_trigger_metadata":{"commit_message":"  ","author":""}}"#,
            ),
            &run(),
        );
        assert_eq!(queued.started_at, None);
        assert_eq!(queued.finished_at, None);
        assert_eq!(queued.commit_message, None);
        assert_eq!(queued.actor, None);
        assert_eq!(queued.trigger, Trigger::Unknown);
        let bare = into_details(
            build(
                r#"{"build_uuid":"u","status":"stopped","created_on":"2026-11-16T10:00:00Z","modified_on":"2026-11-16T10:01:00Z"}"#,
            ),
            &run(),
        );
        assert_eq!(
            bare.started_at.unwrap().to_rfc3339(),
            "2026-11-16T10:00:00+00:00"
        );
        assert_eq!(
            bare.finished_at.unwrap().to_rfc3339(),
            "2026-11-16T10:01:00+00:00"
        );
    }
}
