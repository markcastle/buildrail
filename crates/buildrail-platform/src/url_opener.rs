//! Opening a provider run page in the user's browser.
//!
//! Run URLs come from network responses, so they are untrusted: only
//! absolute `https://` URLs with a host and no embedded credentials are
//! passed to the OS. Anything else (`file:`, `javascript:`, custom app
//! schemes, `http:`) is refused before the browser is involved, so a
//! malicious or compromised API response cannot make "activate entry" run a
//! local program.

use std::sync::{Mutex, PoisonError};

use thiserror::Error;

/// Why a URL was not opened.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpenUrlError {
    /// Not parseable as an absolute URL, or contains whitespace/control
    /// characters.
    #[error("not a valid URL")]
    Invalid,
    /// Parsed, but the scheme is not `https`.
    #[error("only https:// links can be opened (got {scheme}:)")]
    NotHttps {
        /// The scheme that was refused.
        scheme: String,
    },
    /// No host (`https:///path`).
    #[error("the link has no host")]
    MissingHost,
    /// `https://user:pass@host` – refused so credentials never reach the
    /// browser history.
    #[error("links with embedded credentials are not opened")]
    EmbeddedCredentials,
    /// The OS could not launch a browser.
    #[error("could not open the browser: {0}")]
    Launch(String),
}

/// Opens web pages for the user.
pub trait UrlOpener: Send + Sync {
    /// Validates `url` with [`validate_https_url`] and opens it.
    fn open(&self, url: &str) -> Result<(), OpenUrlError>;
}

/// Checks that `raw` is a safe `https://` URL and returns it normalised.
/// The original string is never echoed in errors (it may carry tokens in
/// its query).
pub fn validate_https_url(raw: &str) -> Result<url::Url, OpenUrlError> {
    if raw.is_empty() || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(OpenUrlError::Invalid);
    }
    let parsed = url::Url::parse(raw).map_err(|_| OpenUrlError::Invalid)?;
    if parsed.scheme() != "https" {
        return Err(OpenUrlError::NotHttps {
            scheme: parsed.scheme().to_owned(),
        });
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(OpenUrlError::MissingHost);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(OpenUrlError::EmbeddedCredentials);
    }
    Ok(parsed)
}

/// Opens URLs in the default browser (via the `webbrowser` crate in its
/// hardened mode, which itself refuses non-web schemes).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemUrlOpener;

impl UrlOpener for SystemUrlOpener {
    fn open(&self, url: &str) -> Result<(), OpenUrlError> {
        let url = validate_https_url(url)?;
        webbrowser::open(url.as_str()).map_err(|err| OpenUrlError::Launch(err.kind().to_string()))
    }
}

/// Records URLs instead of opening them, after the same validation as
/// [`SystemUrlOpener`]. For tests and the UI's tests.
#[derive(Debug, Default)]
pub struct RecordingUrlOpener {
    opened: Mutex<Vec<String>>,
}

impl RecordingUrlOpener {
    /// An opener that has opened nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every URL accepted so far (normalised), oldest first.
    pub fn opened(&self) -> Vec<String> {
        self.opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl UrlOpener for RecordingUrlOpener {
    fn open(&self, url: &str) -> Result<(), OpenUrlError> {
        let url = validate_https_url(url)?;
        self.opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(url.into());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_provider_run_urls() {
        for url in [
            "https://github.com/acme/app/actions/runs/123456789",
            "https://dash.cloudflare.com/abc123/workers/services/view/api/production/builds/9f",
            "https://github.com/acme/app/actions/runs/1?check_suite_focus=true#step:3:1",
            "HTTPS://GitHub.com/x",
        ] {
            assert!(validate_https_url(url).is_ok(), "{url}");
        }
        assert_eq!(
            validate_https_url("HTTPS://GitHub.com/x").unwrap().as_str(),
            "https://github.com/x"
        );
    }

    #[test]
    fn refuses_everything_else() {
        let not_https = |s: &str| OpenUrlError::NotHttps {
            scheme: s.to_owned(),
        };
        assert_eq!(
            validate_https_url("http://github.com"),
            Err(not_https("http"))
        );
        assert_eq!(
            validate_https_url("file:///etc/passwd"),
            Err(not_https("file"))
        );
        assert_eq!(
            validate_https_url("javascript:alert(1)"),
            Err(not_https("javascript"))
        );
        assert_eq!(
            validate_https_url("ms-settings:display"),
            Err(not_https("ms-settings"))
        );
        assert_eq!(
            validate_https_url("/relative/path"),
            Err(OpenUrlError::Invalid)
        );
        assert_eq!(validate_https_url(""), Err(OpenUrlError::Invalid));
        assert_eq!(
            validate_https_url(" https://github.com"),
            Err(OpenUrlError::Invalid)
        );
        assert_eq!(
            validate_https_url("https://github.com/a b"),
            Err(OpenUrlError::Invalid)
        );
        assert_eq!(
            validate_https_url("https://github.com/\n"),
            Err(OpenUrlError::Invalid)
        );
        assert_eq!(validate_https_url("https://"), Err(OpenUrlError::Invalid));
        assert_eq!(
            validate_https_url("https://user:pw@github.com/"),
            Err(OpenUrlError::EmbeddedCredentials)
        );
        assert_eq!(
            validate_https_url("https://user@github.com/"),
            Err(OpenUrlError::EmbeddedCredentials)
        );
    }

    #[test]
    fn errors_do_not_echo_the_url() {
        let err = validate_https_url("https://u:SECRET@x.com").unwrap_err();
        assert!(!err.to_string().contains("SECRET"));
        assert!(OpenUrlError::Launch("x".into())
            .to_string()
            .contains("browser"));
        assert!(OpenUrlError::MissingHost.to_string().contains("host"));
        assert!(OpenUrlError::Invalid.to_string().contains("valid"));
    }

    #[test]
    fn recording_opener_validates_and_records() {
        let opener = RecordingUrlOpener::new();
        opener.open("https://github.com/a").unwrap();
        assert!(opener.open("file:///x").is_err());
        let as_trait: &dyn UrlOpener = &opener;
        as_trait.open("https://github.com/b").unwrap();
        assert_eq!(
            opener.opened(),
            vec!["https://github.com/a", "https://github.com/b"]
        );
    }

    #[test]
    fn system_opener_refuses_before_launching() {
        // Only invalid URLs here: a valid one would really open a browser.
        assert_eq!(
            SystemUrlOpener.open("file:///etc/passwd"),
            Err(OpenUrlError::NotHttps {
                scheme: "file".into()
            })
        );
    }
}
