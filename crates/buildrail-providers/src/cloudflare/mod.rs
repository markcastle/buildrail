//! Cloudflare Workers Builds adapter.
//!
//! * [`CloudflareBuilds`] implements
//!   [`RunSource`](buildrail_core::ports::RunSource).
//! * [`CloudflareCatalog`] implements the Cloudflare catalogue traits.
//!
//! # Endpoints used
//!
//! * `GET /accounts/{account_id}/workers/scripts` – resolves the Worker
//!   *name* to its immutable *tag*; the Builds API only accepts the tag
//!   ("Resource not found" otherwise). Cached per account/name/token.
//! * `GET /accounts/{account_id}/builds/workers/{script_tag}/builds` –
//!   recent builds (`page`, `per_page` ≤ 200; builds older than 90 days are
//!   not returned). There is no branch query parameter, so branch filtering
//!   is done client-side.
//! * `GET /user/tokens/verify`, `GET /accounts`, plus the scripts list – for
//!   the catalogue.
//!
//! # Token permissions (read-only)
//!
//! The Builds API requires a **user** API token (account-owned tokens get
//! "Invalid token"). Grant, for the accounts to watch:
//!
//! * **Workers CI: Read** – list builds. (The OpenAPI spec lists "Workers CI
//!   Read" or "Workers CI Write" for the builds endpoints; the Builds API
//!   guide's dashboard wording is "Workers Builds Configuration", for which
//!   it suggests Edit because it also covers changing build settings.
//!   BuildRail only reads.)
//! * **Workers Scripts: Read** – resolve Worker names to tags and list
//!   Workers.
//! * `GET /accounts` and `GET /user/tokens/verify` need no extra
//!   permission; the former lists the accounts the token is scoped to.
//!
//! # API gotchas
//!
//! * Every response is wrapped in `{ success, errors, messages, result,
//!   result_info }`.
//! * Rate limit: 429 with `retry-after`.
//! * Some invalid-token answers are HTTP 400 with an auth error code in
//!   `errors`; these are mapped to `AccessDenied`.

mod catalog;
mod runs;
pub mod status;

use buildrail_core::ports::ProviderError;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::http::decode;

pub use catalog::CloudflareCatalog;
pub use runs::{CloudflareBuilds, DEFAULT_BUILD_LIMIT};

/// Production Cloudflare API base URL (v4).
pub const CLOUDFLARE_API_URL: &str = "https://api.cloudflare.com/client/v4";

/// Headers sent with every Cloudflare request (besides auth and user agent).
const HEADERS: &[(&str, &str)] = &[("accept", "application/json")];

/// Cloudflare's `{ success, errors, result, result_info }` envelope.
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    errors: Vec<ApiMessage>,
    result: Option<T>,
    result_info: Option<ResultInfo>,
}

#[derive(Debug, Deserialize)]
struct ApiMessage {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

/// Pagination info. Fields differ between Cloudflare products, so all are
/// optional.
#[derive(Debug, Default, Clone, Copy, Deserialize)]
struct ResultInfo {
    total_pages: Option<u32>,
    total_count: Option<u32>,
}

/// Decodes an envelope and returns its `result` and `result_info`. A
/// `success: false` on a 2xx is reported as [`ProviderError::Malformed`]
/// with Cloudflare's own error messages (which never contain the token).
fn open_envelope<T: DeserializeOwned>(
    bytes: &[u8],
) -> Result<(Option<T>, ResultInfo), ProviderError> {
    let envelope: Envelope<T> = decode(bytes)?;
    if !envelope.success {
        let reasons = envelope
            .errors
            .iter()
            .map(|e| format!("{} {}", e.code, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ProviderError::Malformed(format!(
            "Cloudflare reported failure: {reasons}"
        )));
    }
    Ok((envelope.result, envelope.result_info.unwrap_or_default()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn envelope_failure_is_malformed_with_reasons() {
        let err = open_envelope::<Vec<u8>>(
            br#"{"success":false,"errors":[{"code":7000,"message":"No route"}],"result":null}"#,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ProviderError::Malformed("Cloudflare reported failure: 7000 No route".into())
        );
    }

    #[test]
    fn envelope_success_without_result() {
        let (result, info) =
            open_envelope::<Vec<u8>>(br#"{"success":true,"errors":[],"result":null}"#).unwrap();
        assert!(result.is_none());
        assert!(info.total_pages.is_none());
    }
}
