//! [`CloudflareCatalog`]: Cloudflare catalogue for the "Add build" picker.

use std::sync::Arc;

use async_trait::async_trait;
use buildrail_core::domain::ProviderKind;
use buildrail_core::ports::{Clock, ProviderError, Secret};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::{open_envelope, CLOUDFLARE_API_URL, HEADERS};
use crate::catalog::{
    CloudflareAccount, CloudflareAccounts, CloudflareTokenCheck, CloudflareTokenStatus,
    CloudflareWorker, CloudflareWorkers, MAX_ACCOUNTS,
};
use crate::http::{endpoint, Transport};

/// Largest `per_page` `GET /accounts` accepts.
const ACCOUNTS_PAGE_SIZE: usize = 50;

/// Cloudflare catalogue: token check, accounts and Workers.
#[derive(Debug)]
pub struct CloudflareCatalog {
    transport: Transport,
    base_url: String,
}

impl CloudflareCatalog {
    /// A catalogue for `api.cloudflare.com`.
    pub fn new(client: reqwest::Client, clock: Arc<dyn Clock>) -> Self {
        Self {
            transport: Transport::new(client, ProviderKind::Cloudflare, clock, HEADERS),
            base_url: CLOUDFLARE_API_URL.to_owned(),
        }
    }

    /// Points the catalogue at another API root (including `/client/v4`).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

#[async_trait]
impl CloudflareTokenCheck for CloudflareCatalog {
    async fn verify_token(
        &self,
        credential: &Secret,
    ) -> Result<CloudflareTokenStatus, ProviderError> {
        let url = endpoint(&self.base_url, &["user", "tokens", "verify"])?;
        let (bytes, _) = self.transport.get_body(url, credential).await?;
        let (result, _) = open_envelope::<WireToken>(&bytes)?;
        let token = result.ok_or_else(|| {
            ProviderError::Malformed("token verification returned no result".into())
        })?;
        Ok(CloudflareTokenStatus {
            active: token.status == "active",
            id: token.id,
            status: token.status,
            expires_on: token.expires_on,
        })
    }
}

#[async_trait]
impl CloudflareAccounts for CloudflareCatalog {
    async fn accounts(&self, credential: &Secret) -> Result<Vec<CloudflareAccount>, ProviderError> {
        let mut out: Vec<CloudflareAccount> = Vec::new();
        let max_pages = MAX_ACCOUNTS.div_ceil(ACCOUNTS_PAGE_SIZE);
        for page in 1..=max_pages {
            let mut url = endpoint(&self.base_url, &["accounts"])?;
            url.query_pairs_mut()
                .append_pair("page", &page.to_string())
                .append_pair("per_page", &ACCOUNTS_PAGE_SIZE.to_string());
            let (bytes, _) = self.transport.get_body(url, credential).await?;
            let (result, info) = open_envelope::<Vec<CloudflareAccount>>(&bytes)?;
            let items = result.unwrap_or_default();
            let short_page = items.len() < ACCOUNTS_PAGE_SIZE;
            out.extend(items);
            let last_by_pages = info
                .total_pages
                .is_some_and(|total| page >= usize::try_from(total).unwrap_or(0));
            let last_by_count = info
                .total_count
                .is_some_and(|total| out.len() >= usize::try_from(total).unwrap_or(0));
            if short_page || last_by_pages || last_by_count {
                break;
            }
        }
        out.truncate(MAX_ACCOUNTS);
        Ok(out)
    }
}

#[async_trait]
impl CloudflareWorkers for CloudflareCatalog {
    async fn workers(
        &self,
        account_id: &str,
        credential: &Secret,
    ) -> Result<Vec<CloudflareWorker>, ProviderError> {
        // This endpoint is not paginated.
        let url = endpoint(
            &self.base_url,
            &["accounts", account_id, "workers", "scripts"],
        )?;
        let (bytes, _) = self.transport.get_body(url, credential).await?;
        let (result, _) = open_envelope::<Vec<WireScript>>(&bytes)?;
        Ok(result
            .unwrap_or_default()
            .into_iter()
            .map(|s| CloudflareWorker {
                name: s.id,
                tag: s.tag,
                modified_on: s.modified_on,
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
struct WireToken {
    id: String,
    status: String,
    expires_on: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct WireScript {
    id: String,
    tag: Option<String>,
    modified_on: Option<DateTime<Utc>>,
}
