//! Salesforce OAuth `client_credentials` exchange with PDK shared-cache
//! caching and proactive refresh on safety margin.
//!
//! The body of the call is the simple form
//!
//! ```text
//! grant_type=client_credentials
//! client_id=<consumer_key>
//! client_secret=<consumer_secret>
//! ```
//!
//! This module exposes both the pure helpers (form/response parsers) and
//! the async `get_token` function used by the request filter. The 401
//! reactive retry is implemented in `client::with_bearer_retry`.

use std::rc::Rc;
use std::time::Duration;

use pdk::cache::Cache;
use pdk::hl::{HttpClient, Service};
use pdk::logger;
use serde::Deserialize;
use thiserror::Error;

use crate::cache::{token_cache_key, CachedToken, AGENTFORCE_TOKEN_PREFIX};

/// Conservative default lifetime we apply when the IdP omits `expires_in`.
const DEFAULT_EXPIRES_IN_SECS: u64 = 300;

/// Path appended to the My Domain URL.
pub const SALESFORCE_TOKEN_PATH: &str = "/services/oauth2/token";

#[derive(Debug, Clone)]
pub struct AgentforceAuthConfig {
    pub consumer_key: String,
    pub consumer_secret: String,
    pub access_token_override: Option<String>,
    /// Used as the cache-key salt and in the Service host disambiguation.
    pub my_domain_url_for_cache_key: String,
    pub cache_safety_margin_seconds: u32,
    /// Per-call HTTP timeout for the OAuth token exchange. Sourced from the
    /// operator-configurable `agentforceRequestTimeoutSeconds` (see client.rs).
    pub request_timeout_secs: u32,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("transport error talking to {endpoint}: {source}")]
    Transport {
        endpoint: &'static str,
        #[source]
        source: anyhow::Error,
    },

    #[error("token endpoint returned HTTP {status}")]
    HttpStatus { status: u32 },

    #[error("token endpoint response was not valid JSON: {0}")]
    BadJson(String),

    #[error("token endpoint response missing access_token")]
    MissingAccessToken,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    /// Lifetime in seconds. All compliant IdPs return this; default to a
    /// conservative 300s when missing so we never cache forever.
    #[serde(default = "default_expires_in")]
    pub expires_in: u64,
    #[serde(default)]
    #[allow(dead_code)]
    pub token_type: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub scope: Option<String>,
}

fn default_expires_in() -> u64 {
    DEFAULT_EXPIRES_IN_SECS
}

/// Build the URL-encoded form body for a `client_credentials` exchange.
pub fn build_form(client_id: &str, client_secret: &str) -> String {
    serde_urlencoded::to_string(&[
        ("grant_type", "client_credentials"),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ])
    .expect("urlencoded serialization is infallible for &str")
}

pub fn parse_response(body: &[u8]) -> Result<TokenResponse, AuthError> {
    let resp: TokenResponse =
        serde_json::from_slice(body).map_err(|e| AuthError::BadJson(e.to_string()))?;
    if resp.access_token.is_empty() {
        return Err(AuthError::MissingAccessToken);
    }
    Ok(resp)
}

/// Redact a token for logging: keep the first 4 chars + the length, never
/// the full secret.
pub fn redact(token: &str) -> String {
    let n = token.chars().count();
    if n <= 8 {
        "***".to_string()
    } else {
        let prefix: String = token.chars().take(4).collect();
        format!("{prefix}...({n} chars)")
    }
}

/// Salesforce OAuth client. Reused per request via `Rc`.
pub struct AgentforceAuth {
    cfg: AgentforceAuthConfig,
    cache: Rc<dyn Cache>,
    /// Salesforce My Domain URL upstream registered as a `format: service`
    /// in `gcl.yaml`.
    my_domain: Rc<Service>,
}

impl AgentforceAuth {
    pub fn new(
        cfg: AgentforceAuthConfig,
        cache: Rc<dyn Cache>,
        my_domain: Rc<Service>,
    ) -> Self {
        Self {
            cfg,
            cache,
            my_domain,
        }
    }

    pub fn cache_key(&self) -> String {
        token_cache_key(
            AGENTFORCE_TOKEN_PREFIX,
            &self.cfg.consumer_key,
            &self.cfg.my_domain_url_for_cache_key,
        )
    }

    /// Get a usable bearer token.
    ///
    /// `force_refresh = true` evicts any cached token and mints a fresh one
    /// unconditionally - used by the 401 reactive retry path.
    pub async fn get_token(
        &self,
        client: &HttpClient,
        now_unix: u64,
        force_refresh: bool,
    ) -> Result<String, AuthError> {
        if let Some(token) = self.cfg.access_token_override.as_deref() {
            logger::warn!("agentforce-auth: using configured access token override");
            return Ok(token.to_string());
        }

        let key = self.cache_key();

        if !force_refresh {
            if let Some(bytes) = self.cache.get(&key) {
                if let Ok(entry) = serde_json::from_slice::<CachedToken>(&bytes) {
                    if !entry.needs_refresh(now_unix, self.cfg.cache_safety_margin_seconds) {
                        logger::debug!("agentforce-auth: cache hit");
                        return Ok(entry.access_token);
                    }
                    logger::debug!(
                        "agentforce-auth: cached token within safety margin, refreshing"
                    );
                }
            }
        } else {
            logger::debug!("agentforce-auth: forced refresh, evicting cache entry");
            self.cache.delete(&key);
        }

        let body = build_form(&self.cfg.consumer_key, &self.cfg.consumer_secret);
        let request = client
            .request(self.my_domain.as_ref())
            .path(SALESFORCE_TOKEN_PATH)
            .headers(vec![
                ("content-type", "application/x-www-form-urlencoded"),
                ("accept", "application/json"),
            ])
            .body(body.as_bytes())
            .timeout(Duration::from_secs(self.cfg.request_timeout_secs as u64))
            .post();

        let response = request.await.map_err(|e| AuthError::Transport {
            endpoint: "salesforce-oauth2",
            source: anyhow::anyhow!(e.to_string()),
        })?;

        let status = response.status_code();
        if !(200..300).contains(&status) {
            logger::error!(
                "agentforce-auth: token endpoint returned HTTP {status}: {}",
                String::from_utf8_lossy(response.body())
            );
            return Err(AuthError::HttpStatus { status });
        }

        let parsed = parse_response(response.body())?;
        let entry = CachedToken::new(parsed.access_token.clone(), now_unix, parsed.expires_in);
        if let Ok(bytes) = serde_json::to_vec(&entry) {
            // Best-effort: a cache miss next request just causes another exchange.
            let _ = self.cache.save(&key, bytes);
        }
        logger::info!(
            "agentforce-auth: minted token {} (expires_in={}s)",
            redact(&parsed.access_token),
            parsed.expires_in
        );
        Ok(parsed.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_body_encodes_grant_and_credentials() {
        let body = build_form("3MVG9.consumerkey", "secret with spaces");
        assert!(body.contains("grant_type=client_credentials"));
        assert!(body.contains("client_id=3MVG9.consumerkey"));
        assert!(body.contains("client_secret=secret+with+spaces"));
    }

    #[test]
    fn parse_response_accepts_minimal_body() {
        let body = br#"{"access_token":"00Dxxx","token_type":"Bearer","expires_in":1799}"#;
        let r = parse_response(body).unwrap();
        assert_eq!(r.access_token, "00Dxxx");
        assert_eq!(r.expires_in, 1799);
    }

    #[test]
    fn parse_response_defaults_expires_in() {
        let body = br#"{"access_token":"x"}"#;
        let r = parse_response(body).unwrap();
        assert_eq!(r.expires_in, DEFAULT_EXPIRES_IN_SECS);
    }

    #[test]
    fn parse_response_rejects_missing_token() {
        let body = br#"{"token_type":"Bearer"}"#;
        let err = parse_response(body).unwrap_err();
        assert!(matches!(err, AuthError::BadJson(_) | AuthError::MissingAccessToken));
    }

    #[test]
    fn parse_response_rejects_empty_token() {
        let body = br#"{"access_token":""}"#;
        let err = parse_response(body).unwrap_err();
        assert!(matches!(err, AuthError::MissingAccessToken));
    }

    #[test]
    fn redact_short_tokens() {
        assert_eq!(redact("short"), "***");
        let r = redact("00Dxxxxxx");
        assert!(r.starts_with("00Dx"));
        assert!(r.contains("(9 chars)"));
    }
}
