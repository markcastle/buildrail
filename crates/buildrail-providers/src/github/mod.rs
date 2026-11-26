//! GitHub Actions adapter.
//!
//! * [`GitHubActions`] implements [`RunSource`](buildrail_core::ports::RunSource)
//!   over `GET /repos/{owner}/{repo}/actions/workflows/{workflow}/runs`.
//! * The same [`GitHubActions`] implements
//!   [`RunDetailsSource`](buildrail_core::ports::RunDetailsSource): one
//!   attempt's title, trigger, timings and jobs, for the details panel
//!   (see `details.rs`; two requests per panel open).
//! * [`GitHubCatalog`] implements the GitHub catalogue traits for the
//!   "Add build" picker.
//!
//! # Token permissions (read-only)
//!
//! * Fine-grained PAT: repository access to the watched repositories with
//!   **Actions: Read** and **Metadata: Read** (metadata is implied and
//!   covers listing repositories and branches).
//! * Classic PAT: `repo` for private repositories; a token with no scopes
//!   can read public repositories. (Classic tokens cannot be read-only for
//!   private repositories; prefer fine-grained tokens.)
//!
//! Authenticated requests get 5,000 requests/hour per user.
//!
//! # API gotchas
//!
//! * `workflow` may be the file name (`ci.yml`) or the numeric ID.
//! * Re-running a workflow keeps the run `id` and bumps `run_attempt`; the
//!   jobs of a given attempt are under `/runs/{id}/attempts/{n}/jobs`.
//! * A 403 can be either a permission problem *or* a rate limit; the
//!   `x-ratelimit-remaining` / `retry-after` headers tell them apart.
//! * Conditional requests answered with 304 are free (not counted against
//!   the rate limit), so [`GitHubActions`] caches `ETag`s.

mod catalog;
mod details;
mod runs;
pub mod status;

pub use catalog::GitHubCatalog;
pub use details::{JOBS_PER_PAGE, MAX_JOB_PAGES};
pub use runs::{GitHubActions, DEFAULT_RUN_LIMIT};

/// Production GitHub REST API base URL.
pub const GITHUB_API_URL: &str = "https://api.github.com";

/// The REST API version BuildRail was written against
/// (`X-GitHub-Api-Version`). Pinning it keeps field semantics stable when
/// GitHub ships a new version.
pub const GITHUB_API_VERSION: &str = "2022-11-28";

/// Headers sent with every GitHub request (besides auth and user agent).
const HEADERS: &[(&str, &str)] = &[
    ("accept", "application/vnd.github+json"),
    ("x-github-api-version", GITHUB_API_VERSION),
];
