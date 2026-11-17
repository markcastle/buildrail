//! Fetching `latest.json` and payloads.

use std::time::Duration;

use async_trait::async_trait;

use crate::policy::{
    ALLOWED_DOWNLOAD_HOSTS, FEED_URL, MAX_MANIFEST_BYTES, RELEASE_DOWNLOAD_PREFIX,
};

/// Time allowed for fetching `latest.json`, redirects included.
pub const MANIFEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Time allowed for downloading a payload. Overrides the shared client's
/// (short, API-sized) request timeout.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Why a fetch failed. Messages carry hosts at most, never full URLs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// Transport failure (DNS, TLS, connection, timeout).
    #[error("network error: {0}")]
    Network(String),
    /// The server answered with a non-success status.
    #[error("the server answered HTTP {0}")]
    Status(u16),
    /// The response is over the size cap.
    #[error("the response is larger than the {max}-byte limit")]
    TooLarge {
        /// The cap that was exceeded, in bytes.
        max: u64,
    },
    /// A redirect led somewhere other than GitHub's release hosts.
    #[error("the download was redirected to an unexpected address ({0})")]
    UnexpectedHost(String),
    /// The address asked for is not a BuildRail release download.
    #[error("the address is not a BuildRail release download")]
    NotAllowed,
}

/// Where updates come from. The real one is [`ReqwestFeed`]; tests use
/// [`crate::testing::StaticFeed`].
#[async_trait]
pub trait Feed: Send + Sync {
    /// The raw bytes of `latest.json` (at most [`MAX_MANIFEST_BYTES`]).
    async fn fetch_manifest(&self) -> Result<Vec<u8>, FetchError>;

    /// The raw bytes at `url`, failing with [`FetchError::TooLarge`]
    /// rather than reading more than `max_bytes`.
    async fn download(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, FetchError>;
}

/// The real feed: GitHub release downloads over the app's shared
/// `reqwest::Client` (rustls).
///
/// Besides the size caps, it refuses to start a download outside
/// [`RELEASE_DOWNLOAD_PREFIX`] and rejects any response whose final
/// address (after redirects) is not HTTPS on one of
/// [`ALLOWED_DOWNLOAD_HOSTS`].
#[derive(Debug, Clone)]
pub struct ReqwestFeed {
    client: reqwest::Client,
    feed_url: String,
    download_prefix: String,
    allowed_hosts: Vec<String>,
    require_https: bool,
}

impl ReqwestFeed {
    /// The production feed at [`FEED_URL`].
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            feed_url: FEED_URL.to_owned(),
            download_prefix: RELEASE_DOWNLOAD_PREFIX.to_owned(),
            allowed_hosts: ALLOWED_DOWNLOAD_HOSTS
                .iter()
                .map(|h| (*h).to_owned())
                .collect(),
            require_https: true,
        }
    }

    /// A feed served by a local test server (plain HTTP on 127.0.0.1).
    #[cfg(test)]
    fn local(client: reqwest::Client, base: &str) -> Self {
        Self {
            client,
            feed_url: format!("{base}/latest/download/latest.json"),
            download_prefix: format!("{base}/download/"),
            allowed_hosts: vec!["127.0.0.1".to_owned()],
            require_https: false,
        }
    }

    fn check_final_url(&self, url: &reqwest::Url) -> Result<(), FetchError> {
        let host = url.host_str().unwrap_or_default();
        let scheme_ok = url.scheme() == "https" || !self.require_https;
        let host_ok = self
            .allowed_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(host));
        if scheme_ok && host_ok {
            Ok(())
        } else {
            Err(FetchError::UnexpectedHost(format!(
                "{}://{host}",
                url.scheme()
            )))
        }
    }

    async fn get(
        &self,
        url: &str,
        max_bytes: u64,
        timeout: Duration,
    ) -> Result<Vec<u8>, FetchError> {
        let mut response = self
            .client
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .map_err(network)?;
        self.check_final_url(response.url())?;
        let status = response.status();
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }
        let too_large = FetchError::TooLarge { max: max_bytes };
        if response.content_length().is_some_and(|len| len > max_bytes) {
            return Err(too_large);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            let total = u64::try_from(body.len() + chunk.len()).unwrap_or(u64::MAX);
            if total > max_bytes {
                return Err(too_large);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

fn network(error: reqwest::Error) -> FetchError {
    let text = if error.is_timeout() {
        "the request timed out".to_owned()
    } else if error.is_connect() {
        "could not connect".to_owned()
    } else {
        error.without_url().to_string()
    };
    FetchError::Network(text)
}

#[async_trait]
impl Feed for ReqwestFeed {
    async fn fetch_manifest(&self) -> Result<Vec<u8>, FetchError> {
        self.get(&self.feed_url, MAX_MANIFEST_BYTES, MANIFEST_TIMEOUT)
            .await
    }

    async fn download(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        if !url.starts_with(&self.download_prefix) {
            return Err(FetchError::NotAllowed);
        }
        self.get(url, max_bytes, DOWNLOAD_TIMEOUT).await
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    async fn server() -> (MockServer, ReqwestFeed) {
        let server = MockServer::start().await;
        let feed = ReqwestFeed::local(reqwest::Client::new(), &server.uri());
        (server, feed)
    }

    #[test]
    fn production_feed_points_at_github() {
        let feed = ReqwestFeed::new(reqwest::Client::new());
        assert_eq!(feed.feed_url, FEED_URL);
        assert!(feed.require_https);
        let ok = reqwest::Url::parse("https://release-assets.githubusercontent.com/x").unwrap();
        assert_eq!(feed.check_final_url(&ok), Ok(()));
        let http = reqwest::Url::parse("http://github.com/x").unwrap();
        assert_eq!(
            feed.check_final_url(&http),
            Err(FetchError::UnexpectedHost("http://github.com".into()))
        );
        let evil = reqwest::Url::parse("https://evil.example/a?token=secret").unwrap();
        let err = feed.check_final_url(&evil).unwrap_err();
        assert!(!err.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn fetches_the_manifest() {
        let (server, feed) = server().await;
        Mock::given(method("GET"))
            .and(path("/latest/download/latest.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        assert_eq!(feed.fetch_manifest().await, Ok(b"{}".to_vec()));
    }

    #[tokio::test]
    async fn reports_status_codes() {
        let (_server, feed) = server().await;
        assert_eq!(feed.fetch_manifest().await, Err(FetchError::Status(404)));
    }

    #[tokio::test]
    async fn caps_the_manifest_size() {
        let (server, feed) = server().await;
        let body = vec![b' '; usize::try_from(MAX_MANIFEST_BYTES).unwrap() + 1];
        Mock::given(path("/latest/download/latest.json"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;
        assert_eq!(
            feed.fetch_manifest().await,
            Err(FetchError::TooLarge {
                max: MAX_MANIFEST_BYTES
            })
        );
    }

    #[tokio::test]
    async fn downloads_within_the_cap_and_refuses_beyond_it() {
        let (server, feed) = server().await;
        Mock::given(path("/download/v1/a.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8; 100]))
            .mount(&server)
            .await;
        let url = format!("{}/download/v1/a.gz", server.uri());
        assert_eq!(feed.download(&url, 100).await.map(|b| b.len()), Ok(100));
        assert_eq!(
            feed.download(&url, 99).await,
            Err(FetchError::TooLarge { max: 99 })
        );
    }

    #[tokio::test]
    async fn caps_streamed_bodies_without_a_length() {
        use futures_like::chunked_body;
        let (server, feed) = server().await;
        Mock::given(path("/download/v1/stream.gz"))
            .respond_with(chunked_body(vec![2u8; 64]))
            .mount(&server)
            .await;
        let url = format!("{}/download/v1/stream.gz", server.uri());
        assert_eq!(
            feed.download(&url, 10).await,
            Err(FetchError::TooLarge { max: 10 })
        );
    }

    #[tokio::test]
    async fn refuses_addresses_outside_the_release_prefix() {
        let (server, feed) = server().await;
        let url = format!("{}/elsewhere/a.gz", server.uri());
        assert_eq!(feed.download(&url, 10).await, Err(FetchError::NotAllowed));
    }

    #[tokio::test]
    async fn refuses_redirects_to_other_hosts() {
        let (server, feed) = server().await;
        // `localhost` is the same machine but not an allowed host name.
        let elsewhere = server.uri().replace("127.0.0.1", "localhost");
        Mock::given(path("/download/v1/r.gz"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{elsewhere}/download/v1/final.gz")),
            )
            .mount(&server)
            .await;
        let url = format!("{}/download/v1/r.gz", server.uri());
        assert_eq!(
            feed.download(&url, 10).await,
            Err(FetchError::UnexpectedHost("http://localhost".into()))
        );
    }

    #[tokio::test]
    async fn network_errors_are_reported_without_the_url() {
        // Bind then drop a listener to get a port nothing listens on.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let feed = ReqwestFeed::local(reqwest::Client::new(), &format!("http://127.0.0.1:{port}"));
        assert_eq!(
            feed.fetch_manifest().await,
            Err(FetchError::Network("could not connect".into()))
        );
    }

    #[tokio::test]
    async fn timeouts_are_named() {
        let (server, _) = server().await;
        Mock::given(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        let feed = ReqwestFeed::local(reqwest::Client::new(), &server.uri());
        let err = feed
            .get(
                &format!("{}/slow", server.uri()),
                10,
                Duration::from_millis(50),
            )
            .await;
        assert_eq!(
            err,
            Err(FetchError::Network("the request timed out".into()))
        );
    }

    #[tokio::test]
    async fn other_transport_errors_keep_reqwest_text() {
        let (_server, feed) = server().await;
        let err = feed
            .get("not a url", 10, MANIFEST_TIMEOUT)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Network(text) if !text.contains("not a url")));
    }

    /// A response whose body has no `Content-Length`, so the streaming cap is exercised.
    mod futures_like {
        use wiremock::{Request, Respond, ResponseTemplate};

        pub struct Chunked(Vec<u8>);

        impl Respond for Chunked {
            fn respond(&self, _: &Request) -> ResponseTemplate {
                ResponseTemplate::new(200)
                    .insert_header("transfer-encoding", "chunked")
                    .set_body_bytes(self.0.clone())
            }
        }

        pub fn chunked_body(bytes: Vec<u8>) -> Chunked {
            Chunked(bytes)
        }
    }
}
