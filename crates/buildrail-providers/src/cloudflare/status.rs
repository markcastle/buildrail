//! Pure mapping from Workers Builds fields to [`BuildStatus`], plus the
//! dashboard link. Every assumption the docs do not pin down lives here, in
//! one small function each, so it is a one-line fix if Cloudflare changes.
//!
//! Source: the Workers Builds API schema ("List builds for a Worker",
//! `GET /accounts/{account_id}/builds/workers/{script_tag}/builds`):
//!
//! * `status`: `queued`, `initializing`, `running`, `stopped`.
//! * `build_outcome` (null until stopped): `success`, `fail`, `skipped`,
//!   `cancelled`, `terminated`.

use buildrail_core::domain::BuildStatus;

/// Cloudflare dashboard origin used for [`dashboard_build_url`].
pub const DASHBOARD_URL: &str = "https://dash.cloudflare.com";

/// Maps a build's `status` / `build_outcome` to a [`BuildStatus`].
///
/// `production_deploy` comes from [`is_production_deploy`]: a successful
/// build that ran the production deploy command is [`BuildStatus::Deployed`]
/// (the deploy is a step *inside* the build, so success means the new
/// version was promoted), anything else successful is
/// [`BuildStatus::Passed`].
pub fn map_build_status(
    status: &str,
    outcome: Option<&str>,
    production_deploy: bool,
) -> BuildStatus {
    match status {
        "queued" => BuildStatus::Queued,
        "initializing" | "running" => BuildStatus::Running,
        "stopped" => map_outcome(outcome, production_deploy),
        _ => BuildStatus::Unavailable,
    }
}

/// Maps a stopped build's `build_outcome`.
///
/// ASSUMPTIONS:
/// * `terminated` (stopped by the platform, e.g. the build time limit) is a
///   failure from the user's point of view.
/// * The event-subscription payloads spell outcomes `failure` / `canceled`
///   while the REST schema says `fail` / `cancelled`; both spellings are
///   accepted.
pub fn map_outcome(outcome: Option<&str>, production_deploy: bool) -> BuildStatus {
    match outcome {
        Some("success") if production_deploy => BuildStatus::Deployed,
        Some("success") => BuildStatus::Passed,
        Some("fail" | "failure" | "terminated") => BuildStatus::Failed,
        Some("cancelled" | "canceled") => BuildStatus::Cancelled,
        // Workers Builds skips a queued build when a newer one for the same
        // trigger is queued.
        Some("skipped") => BuildStatus::Skipped,
        _ => BuildStatus::Unavailable,
    }
}

/// Whether a build ran on a *production* trigger with a deploying command,
/// i.e. whether its success means "confirmed deployed".
///
/// From the docs: a production trigger has exactly one `branch_includes`
/// entry without wildcards and no `branch_excludes`; legacy non-production
/// triggers are `["*"]` minus excludes. Production builds run the deploy
/// command (default `npx wrangler deploy`, which promotes the new version
/// to the active deployment). Non-production builds run
/// `wrangler versions upload` or `wrangler preview`, which do not.
///
/// ASSUMPTION: when the deploy command is unknown we trust the trigger
/// shape (the default production command deploys); a custom production
/// command containing `versions upload` or `preview` is treated as not
/// deploying. Without trigger information we never claim `Deployed`.
pub fn is_production_deploy(
    branch_includes: Option<&[String]>,
    branch_excludes: &[String],
    deploy_command: Option<&str>,
) -> bool {
    let production_trigger = matches!(
        branch_includes,
        Some([only]) if !only.contains('*')
    ) && branch_excludes.is_empty();
    let deploys = deploy_command.is_none_or(|command| {
        !(command.contains("versions upload") || command.contains("wrangler preview"))
    });
    production_trigger && deploys
}

/// Dashboard page of one build.
///
/// ASSUMPTION: the dashboard's build-detail route is
/// `/{account_id}/workers/services/view/{worker}/production/builds/{uuid}`;
/// it is not part of the documented API.
pub fn dashboard_build_url(account_id: &str, worker: &str, build_uuid: &str) -> String {
    format!(
        "{DASHBOARD_URL}/{}/workers/services/view/{}/production/builds/{}",
        encode(account_id),
        encode(worker),
        encode(build_uuid)
    )
}

/// Percent-encodes one path segment (Worker names are `[a-z0-9-]` in
/// practice, but stay safe).
fn encode(segment: &str) -> String {
    url::form_urlencoded::byte_serialize(segment.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[cfg(test)]
mod tests {
    use super::*;
    use BuildStatus::*;

    #[test]
    fn active_states() {
        assert_eq!(map_build_status("queued", None, true), Queued);
        assert_eq!(map_build_status("initializing", None, true), Running);
        assert_eq!(map_build_status("running", None, false), Running);
        assert_eq!(map_build_status("mystery", None, false), Unavailable);
    }

    #[test]
    fn stopped_outcomes() {
        let cases = [
            (Some("success"), true, Deployed),
            (Some("success"), false, Passed),
            (Some("fail"), true, Failed),
            (Some("failure"), false, Failed),
            (Some("terminated"), false, Failed),
            (Some("cancelled"), false, Cancelled),
            (Some("canceled"), false, Cancelled),
            (Some("skipped"), true, Skipped),
            (Some("new_value"), true, Unavailable),
            (None, true, Unavailable),
        ];
        for (outcome, prod, expected) in cases {
            assert_eq!(
                map_build_status("stopped", outcome, prod),
                expected,
                "{outcome:?} prod={prod}"
            );
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn production_deploy_rules() {
        let main = v(&["main"]);
        assert!(is_production_deploy(Some(&main), &[], None));
        assert!(is_production_deploy(
            Some(&main),
            &[],
            Some("npx wrangler deploy")
        ));
        assert!(!is_production_deploy(
            Some(&main),
            &[],
            Some("npx wrangler versions upload")
        ));
        assert!(!is_production_deploy(
            Some(&main),
            &[],
            Some("npx wrangler preview")
        ));
        assert!(!is_production_deploy(Some(&v(&["*"])), &main, None));
        assert!(!is_production_deploy(Some(&v(&["*"])), &[], None));
        assert!(!is_production_deploy(Some(&v(&["a", "b"])), &[], None));
        assert!(!is_production_deploy(Some(&[]), &[], None));
        assert!(!is_production_deploy(None, &[], None));
    }

    #[test]
    fn dashboard_url_is_encoded() {
        assert_eq!(
            dashboard_build_url("acc", "my worker", "u-1"),
            "https://dash.cloudflare.com/acc/workers/services/view/my%20worker/production/builds/u-1"
        );
    }
}
