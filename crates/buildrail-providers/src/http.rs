//! Shared HTTP plumbing for every provider adapter.
//!
//! Both GitHub and Cloudflare are "bearer token + JSON over HTTPS" APIs, so
//! one small [`Transport`] does the request building and, more importantly,
//! the *error classification*: every adapter maps HTTP failures to
//! [`ProviderError`] the same way, which is what lets core decide whether to
//! reconnect, back off or simply retry later.
//!
//! Secrets: the credential is only ever placed in the `Authorization` header
//! (marked sensitive so `reqwest` redacts it from its own `Debug` output).
//! Errors are built from status codes, header values and decoder messages,
//! never from the request, so a token cannot end up in an error string.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use buildrail_core::domain::ProviderKind;
use buildrail_core::ports::{Clock, ProviderError, Secret};
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, ETAG, IF_NONE_MATCH};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use url::Url;

/// `User-Agent` sent with every request. Deliberately generic: it names the
/// app and version only, never the user or machine. GitHub rejects requests
/// without a `User-Agent`.
pub const USER_AGENT: &str = concat!("buildrail/", env!("CARGO_PKG_VERSION"));

/// Connect timeout of [`default_http_client`].
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole-request timeout of [`default_http_client`].
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Builds the `reqwest::Client` BuildRail should share between all adapters:
/// rustls (no OpenSSL), gzip, a 10 s connect and 30 s total timeout and the
/// generic [`USER_AGENT`].
///
/// Share one client for the whole app so connections (and HTTP/2 sessions)
/// to `api.github.com` / `api.cloudflare.com` are pooled.
pub fn default_http_client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .gzip(true)
        .build()
        .map_err(network_error)
}

/// A successful (2xx or 304) answer.
#[derive(Debug)]
pub(crate) enum Fetched {
    /// A 2xx with its body.
    Body {
        /// Raw response body.
        bytes: Vec<u8>,
        /// The `ETag` header, if the provider sent one.
        etag: Option<String>,
        /// All response headers (pagination links live here).
        headers: HeaderMap,
    },
    /// 304: the conditional request matched; reuse the cached body.
    NotModified,
}

/// Provider-flavoured wrapper around a shared `reqwest::Client`.
#[derive(Clone)]
pub(crate) struct Transport {
    client: reqwest::Client,
    provider: ProviderKind,
    clock: Arc<dyn Clock>,
    headers: &'static [(&'static str, &'static str)],
}

impl Transport {
    /// `headers` are added to every request (e.g. GitHub's API version).
    pub(crate) fn new(
        client: reqwest::Client,
        provider: ProviderKind,
        clock: Arc<dyn Clock>,
        headers: &'static [(&'static str, &'static str)],
    ) -> Self {
        Self {
            client,
            provider,
            clock,
            headers,
        }
    }

    /// The injected clock; adapters use it for `observed_at`.
    pub(crate) fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// `GET url` with the bearer `credential`, optionally conditional on
    /// `if_none_match`. Non-success statuses are classified into
    /// [`ProviderError`].
    pub(crate) async fn get(
        &self,
        url: Url,
        credential: &Secret,
        if_none_match: Option<&str>,
    ) -> Result<Fetched, ProviderError> {
        let mut auth =
            HeaderValue::try_from(format!("Bearer {}", credential.expose())).map_err(|_| {
                ProviderError::AccessDenied {
                    provider: self.provider,
                }
            })?;
        auth.set_sensitive(true);

        let mut request = self
            .client
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(AUTHORIZATION, auth);
        for (name, value) in self.headers {
            request = request.header(*name, *value);
        }
        if let Some(tag) = if_none_match {
            request = request.header(IF_NONE_MATCH, tag);
        }

        let response = request.send().await.map_err(network_error)?;
        let status = response.status();
        let headers = response.headers().clone();

        if status == StatusCode::NOT_MODIFIED && if_none_match.is_some() {
            return Ok(Fetched::NotModified);
        }
        let bytes = response.bytes().await.map_err(network_error)?.to_vec();
        if status.is_success() {
            let etag = header_str(&headers, ETAG.as_str()).map(str::to_owned);
            return Ok(Fetched::Body {
                bytes,
                etag,
                headers,
            });
        }
        Err(self.classify(status, &headers, &bytes))
    }

    /// Like [`Transport::get`] without a conditional header, returning the
    /// body and headers.
    pub(crate) async fn get_body(
        &self,
        url: Url,
        credential: &Secret,
    ) -> Result<(Vec<u8>, HeaderMap), ProviderError> {
        match self.get(url, credential, None).await? {
            Fetched::Body { bytes, headers, .. } => Ok((bytes, headers)),
            // Unreachable in practice: 304 is only honoured for conditional
            // requests. Treat it as an unexpected status rather than panic.
            Fetched::NotModified => Err(ProviderError::Server {
                provider: self.provider,
                status: StatusCode::NOT_MODIFIED.as_u16(),
            }),
        }
    }

    fn classify(&self, status: StatusCode, headers: &HeaderMap, body: &[u8]) -> ProviderError {
        let provider = self.provider;
        if let Some(retry_after) = rate_limit(status, headers, self.clock.now()) {
            return ProviderError::RateLimited {
                provider,
                retry_after,
            };
        }
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                ProviderError::AccessDenied { provider }
            }
            StatusCode::NOT_FOUND => ProviderError::NotFound { provider },
            StatusCode::BAD_REQUEST
                if provider == ProviderKind::Cloudflare && is_cloudflare_auth_error(body) =>
            {
                ProviderError::AccessDenied { provider }
            }
            _ => ProviderError::Server {
                provider,
                status: status.as_u16(),
            },
        }
    }
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

/// Cloudflare error codes that mean "this token is malformed, unknown or of
/// the wrong kind". Cloudflare answers some of these with HTTP 400 instead
/// of 401/403.
///
/// ASSUMPTION (not listed in one place in the docs; collected from the
/// Cloudflare API's observed behaviour): 1000 "Invalid API Token", 6003
/// "Invalid request headers", 6111 "Invalid format for Authorization
/// header", 9106/9109 "Missing/Invalid access token", 10000/10001
/// "Authentication error".
const CLOUDFLARE_AUTH_ERROR_CODES: &[i64] = &[1000, 6003, 6111, 9106, 9109, 10000, 10001];

fn is_cloudflare_auth_error(body: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        errors: Vec<Code>,
    }
    #[derive(Deserialize)]
    struct Code {
        #[serde(default)]
        code: i64,
    }
    serde_json::from_slice::<Envelope>(body)
        .map(|e| {
            e.errors
                .iter()
                .any(|c| CLOUDFLARE_AUTH_ERROR_CODES.contains(&c.code))
        })
        .unwrap_or(false)
}

/// Decides whether a response is a rate-limit answer and, if so, how long
/// to wait.
///
/// * 429 is always a rate limit (both providers).
/// * 403 is a rate limit when GitHub says so: `x-ratelimit-remaining: 0`
///   (primary limit) or a `retry-after` header (secondary limit). Any other
///   403 is a permission problem.
///
/// The wait is taken from `retry-after` (seconds, or an HTTP date), else
/// from `x-ratelimit-reset` (epoch seconds) relative to `now`, else unknown.
pub(crate) fn rate_limit(
    status: StatusCode,
    headers: &HeaderMap,
    now: DateTime<Utc>,
) -> Option<Option<Duration>> {
    let retry_after = header_str(headers, "retry-after");
    let exhausted = header_str(headers, "x-ratelimit-remaining").map(str::trim) == Some("0");
    let limited = status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::FORBIDDEN && (exhausted || retry_after.is_some()));
    if !limited {
        return None;
    }
    let from_retry_after = retry_after.and_then(|value| parse_retry_after(value, now));
    let from_reset = || {
        header_str(headers, "x-ratelimit-reset")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .map(|reset| until(now, reset))
    };
    Some(from_retry_after.or_else(from_reset))
}

fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|at| until(now, at.timestamp()))
}

fn until(now: DateTime<Utc>, epoch_seconds: i64) -> Duration {
    let seconds = epoch_seconds.saturating_sub(now.timestamp()).max(0);
    Duration::from_secs(u64::try_from(seconds).unwrap_or(0))
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Maps a transport failure to [`ProviderError::Network`]. The URL is
/// stripped (it never holds the token, but keeps messages short) and only
/// the error chain's text is kept.
pub(crate) fn network_error(err: reqwest::Error) -> ProviderError {
    let kind = if err.is_timeout() {
        "timed out"
    } else if err.is_connect() {
        "could not connect"
    } else {
        "request failed"
    };
    let err = err.without_url();
    let mut message = format!("{kind}: {err}");
    // `reqwest::Error`'s Display hides the root cause (DNS, TLS, …); append
    // the innermost one, which is what helps a user.
    let mut source = std::error::Error::source(&err);
    let mut root = None;
    while let Some(inner) = source {
        root = Some(inner.to_string());
        source = inner.source();
    }
    if let Some(root) = root {
        message.push_str(" (");
        message.push_str(&root);
        message.push(')');
    }
    ProviderError::Network(message)
}

/// Decodes a JSON body into `T`, mapping failures to
/// [`ProviderError::Malformed`].
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ProviderError> {
    serde_json::from_slice(bytes).map_err(|e| ProviderError::Malformed(e.to_string()))
}

/// Appends percent-encoded path `segments` to `base` (whose own path, such
/// as Cloudflare's `/client/v4`, is kept).
pub(crate) fn endpoint(base: &str, segments: &[&str]) -> Result<Url, ProviderError> {
    let mut url = parse_base(base)?;
    url.path_segments_mut()
        .map_err(|()| ProviderError::Malformed(format!("unusable base URL {base}")))?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}

/// Parses a configured base URL.
pub(crate) fn parse_base(base: &str) -> Result<Url, ProviderError> {
    Url::parse(base).map_err(|e| ProviderError::Malformed(format!("invalid base URL: {e}")))
}

/// Follows an RFC 8288 `Link: <…>; rel="next"` header (GitHub pagination).
///
/// Only links with the same scheme, host and port as `base` are followed,
/// so a hostile or misconfigured response can never make us send the token
/// to another host.
pub(crate) fn next_link(headers: &HeaderMap, base: &Url) -> Option<Url> {
    let value = header_str(headers, "link")?;
    value.split(',').find_map(|part| {
        let mut pieces = part.split(';');
        let target = pieces.next()?.trim();
        let is_next = pieces.any(|p| {
            let p = p.trim();
            p == "rel=\"next\"" || p == "rel=next"
        });
        if !is_next {
            return None;
        }
        let url = Url::parse(target.strip_prefix('<')?.strip_suffix('>')?).ok()?;
        (url.origin() == base.origin()).then_some(url)
    })
}

/// A stable, non-reversible identifier of a credential, used to key caches
/// so one token's cached answers are never served to another. The secret
/// itself is never stored.
pub(crate) fn fingerprint(secret: &Secret) -> u64 {
    let mut hasher = DefaultHasher::new();
    secret.expose().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        map
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    #[test]
    fn plain_403_is_not_a_rate_limit() {
        assert_eq!(
            rate_limit(StatusCode::FORBIDDEN, &headers(&[]), now()),
            None
        );
        let remaining = headers(&[("x-ratelimit-remaining", "12")]);
        assert_eq!(rate_limit(StatusCode::FORBIDDEN, &remaining, now()), None);
    }

    #[test]
    fn retry_after_seconds_wins_over_reset() {
        let h = headers(&[
            ("retry-after", "30"),
            ("x-ratelimit-reset", &(now().timestamp() + 600).to_string()),
        ]);
        assert_eq!(
            rate_limit(StatusCode::TOO_MANY_REQUESTS, &h, now()),
            Some(Some(Duration::from_secs(30)))
        );
    }

    #[test]
    fn retry_after_http_date() {
        let h = headers(&[("retry-after", "Thu, 01 Jan 2026 00:01:00 GMT")]);
        assert_eq!(
            rate_limit(StatusCode::TOO_MANY_REQUESTS, &h, now()),
            Some(Some(Duration::from_secs(60)))
        );
    }

    #[test]
    fn reset_in_the_past_is_zero_and_garbage_is_unknown() {
        let past = headers(&[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", &(now().timestamp() - 5).to_string()),
        ]);
        assert_eq!(
            rate_limit(StatusCode::FORBIDDEN, &past, now()),
            Some(Some(Duration::ZERO))
        );
        let garbage = headers(&[("retry-after", "soon")]);
        assert_eq!(
            rate_limit(StatusCode::TOO_MANY_REQUESTS, &garbage, now()),
            Some(None)
        );
    }

    #[test]
    fn next_link_only_follows_same_origin() {
        let base = Url::parse("https://api.github.com").unwrap();
        let h = headers(&[(
            "link",
            "<https://api.github.com/user/repos?page=2>; rel=\"next\", <https://api.github.com/user/repos?page=5>; rel=\"last\"",
        )]);
        assert_eq!(
            next_link(&h, &base).unwrap().as_str(),
            "https://api.github.com/user/repos?page=2"
        );
        let evil = headers(&[("link", "<https://evil.example/x>; rel=\"next\"")]);
        assert_eq!(next_link(&evil, &base), None);
        let last_only = headers(&[("link", "<https://api.github.com/x>; rel=\"last\"")]);
        assert_eq!(next_link(&last_only, &base), None);
        assert_eq!(next_link(&headers(&[]), &base), None);
    }

    #[test]
    fn endpoint_keeps_base_path_and_encodes_segments() {
        let url = endpoint(
            "https://api.cloudflare.com/client/v4",
            &["accounts", "a b/c"],
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://api.cloudflare.com/client/v4/accounts/a%20b%2Fc"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:1/", &["user"]).unwrap().as_str(),
            "http://127.0.0.1:1/user"
        );
        assert!(matches!(
            endpoint("mailto:x@example.com", &["x"]),
            Err(ProviderError::Malformed(_))
        ));
        assert!(matches!(
            endpoint("not a url", &["x"]),
            Err(ProviderError::Malformed(_))
        ));
    }

    #[test]
    fn fingerprint_differs_per_secret() {
        assert_ne!(
            fingerprint(&Secret::new("a")),
            fingerprint(&Secret::new("b"))
        );
        assert_eq!(
            fingerprint(&Secret::new("a")),
            fingerprint(&Secret::new("a"))
        );
    }

    #[test]
    fn cloudflare_auth_error_detection() {
        assert!(is_cloudflare_auth_error(
            br#"{"success":false,"errors":[{"code":6003,"message":"Invalid request headers"}]}"#
        ));
        assert!(!is_cloudflare_auth_error(
            br#"{"success":false,"errors":[{"code":7003,"message":"No route"}]}"#
        ));
        assert!(!is_cloudflare_auth_error(b"not json"));
    }
}
