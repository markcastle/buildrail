//! Shared helpers for the integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use buildrail_core::ports::{Clock, Secret};
use chrono::{DateTime, Duration, TimeZone, Utc};

/// The token used in tests. Distinctive so leak checks are meaningful.
pub const TOKEN: &str = "ghp_SUPERSECRETtoken1234567890abcdef";

pub fn secret() -> Secret {
    Secret::new(TOKEN)
}

/// A clock tests can move forward.
#[derive(Debug)]
pub struct TestClock(Mutex<DateTime<Utc>>);

impl TestClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(
            Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap(),
        )))
    }

    pub fn advance(&self, seconds: i64) {
        let mut now = self.0.lock().unwrap();
        *now += Duration::seconds(seconds);
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

pub fn fixture(path: &str) -> String {
    let full = format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{full}: {e}"))
}

pub fn json(path: &str) -> serde_json::Value {
    serde_json::from_str(&fixture(path)).unwrap()
}

pub fn client() -> reqwest::Client {
    buildrail_providers::default_http_client().unwrap()
}

/// Asserts the token appears nowhere in an error's Display or Debug text.
pub fn assert_no_token<E: std::fmt::Display + std::fmt::Debug>(err: &E) {
    let text = format!("{err} {err:?}");
    assert!(!text.contains(TOKEN), "token leaked: {text}");
    assert!(!text.contains("SUPERSECRET"), "token leaked: {text}");
}
