//! Agentforce Agents API client.
//!
//! Three operations are wired up; v1 only uses the synchronous send path.
//!
//!   * `start_session(agent_id, ext_session_key)`
//!     `POST /agents/{agent_id}/sessions`     -> 200 `{ sessionId, ... }`
//!   * `send_message(session_id, text, sequence_id)`
//!     `POST /sessions/{session_id}/messages` -> 200 `{ messages, _links }`
//!   * `end_session(session_id)`
//!     `DELETE /sessions/{session_id}`        -> 204
//!
//! Each call goes through the proactive-token / 401-reactive-retry pattern
//! described in the design doc: a single 401 from Agentforce evicts the
//! cached token, mints a fresh one, and retries the same call exactly once
//! before surfacing `ClientError::AuthRejected`.

use std::rc::Rc;
use std::time::Duration;

use pdk::hl::{HttpClient, Service};
use pdk::logger;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use urlencoding;

use crate::agentforce::auth::{AgentforceAuth, AuthError};

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("auth error: {0}")]
    Auth(#[from] AuthError),

    #[error("transport error talking to Agentforce: {source}")]
    Transport {
        #[source]
        source: anyhow::Error,
    },

    #[error("Agentforce returned HTTP {status} on {operation}: {body}")]
    HttpStatus {
        operation: &'static str,
        status: u32,
        body: String,
    },

    #[error("Agentforce response body was not valid JSON: {0}")]
    BadJson(String),

    #[error("Agentforce response missing field: {0}")]
    MissingField(&'static str),

    #[error("Agentforce credentials rejected after refresh (HTTP 401)")]
    AuthRejected,
}

#[derive(Debug, Clone, Serialize)]
struct StartSessionRequest<'a> {
    #[serde(rename = "externalSessionKey")]
    external_session_key: &'a str,
    #[serde(rename = "instanceConfig")]
    instance_config: InstanceConfig<'a>,
    #[serde(rename = "featureSupport")]
    feature_support: &'static str,
    #[serde(rename = "streamingCapabilities")]
    streaming_capabilities: StreamingCapabilities,
    #[serde(rename = "bypassUser")]
    bypass_user: bool,
}

#[derive(Debug, Clone, Serialize)]
struct InstanceConfig<'a> {
    endpoint: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct StreamingCapabilities {
    #[serde(rename = "chunkTypes")]
    chunk_types: Vec<&'static str>,
}

impl Default for StreamingCapabilities {
    fn default() -> Self {
        Self {
            chunk_types: vec!["Text"],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StartSessionResponse {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(default)]
    pub messages: Vec<AgentforceMessage>,
}

#[derive(Debug, Clone, Serialize)]
struct SendMessageRequest<'a> {
    message: SendMessageBody<'a>,
}

#[derive(Debug, Clone, Serialize)]
struct SendMessageBody<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "sequenceId")]
    sequence_id: u32,
    text: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SendMessageResponse {
    #[serde(default)]
    pub messages: Vec<AgentforceMessage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentforceMessage {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    /// Some Agentforce response shapes nest the actual text under `result`
    /// or other fields; we preserve the full JSON so the mapping layer can
    /// reach in without us having to model every variant here.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub struct AgentforceClient {
    auth: Rc<AgentforceAuth>,
    api: Rc<Service>,
    /// Base path extracted from `agentforceApiUrl` at policy load time
    /// (e.g. `/einstein/ai-agent/v1` for `https://api.salesforce.com/einstein/ai-agent/v1`).
    /// PDK's `RequestBuilder::path` *replaces* the registered Service URI's path
    /// instead of appending to it (see pdk-classy `client.rs:310`), so we have
    /// to prepend this manually on every outbound call. Empty when the
    /// configured URL is host-only.
    api_base_path: String,
    /// Public Salesforce my-domain URL used as `instanceConfig.endpoint`
    /// in start-session calls. Not the same as the OAuth Service - this
    /// value is passed by reference to Agentforce, not used to dispatch.
    my_domain_url_value: String,
    agent_id: String,
    bypass_user: bool,
    /// Per-call HTTP timeout applied to every outbound Agentforce dispatch.
    /// Sourced from the operator-configurable `agentforceRequestTimeoutSeconds`
    /// (clamped to 1..=290, default 180). The PDK HTTP client defaults to only
    /// 10s, which is too low for long RAG turns and surfaces as a spurious 504.
    request_timeout_secs: u64,
}

impl AgentforceClient {
    pub fn new(
        auth: Rc<AgentforceAuth>,
        api: Rc<Service>,
        api_base_path: String,
        my_domain_url_value: String,
        agent_id: String,
        bypass_user: bool,
        request_timeout_secs: u32,
    ) -> Self {
        Self {
            auth,
            api,
            api_base_path: normalize_base_path(&api_base_path),
            my_domain_url_value,
            agent_id,
            bypass_user,
            request_timeout_secs: request_timeout_secs as u64,
        }
    }

    /// Compose the full request path by joining the configured API base path
    /// (typically `/einstein/ai-agent/v1`) with the per-call relative path
    /// (e.g. `/agents/{id}/sessions`). Always produces a leading `/` and
    /// avoids `//` at the seam.
    fn full_path(&self, rel: &str) -> String {
        let rel_norm = if rel.starts_with('/') {
            rel.to_string()
        } else {
            format!("/{rel}")
        };
        if self.api_base_path.is_empty() {
            rel_norm
        } else {
            format!("{}{}", self.api_base_path, rel_norm)
        }
    }

    pub async fn start_session(
        &self,
        client: &HttpClient,
        external_session_key: &str,
        now_unix: u64,
    ) -> Result<StartSessionResponse, ClientError> {
        let body_struct = StartSessionRequest {
            external_session_key,
            instance_config: InstanceConfig {
                endpoint: self.my_domain_url_value.as_str(),
            },
            feature_support: "Sync",
            streaming_capabilities: StreamingCapabilities::default(),
            bypass_user: self.bypass_user,
        };
        let body =
            serde_json::to_vec(&body_struct).map_err(|e| ClientError::BadJson(e.to_string()))?;
        let path = self.full_path(&format!(
            "/agents/{}/sessions",
            urlencoding::encode(&self.agent_id)
        ));

        // First attempt with the proactively-fresh token.
        let token = self.auth.get_token(client, now_unix, false).await?;
        match self.post_start_session(client, &path, &body, &token).await {
            Ok(r) => Ok(r),
            Err(ClientError::HttpStatus { status: 401, .. }) => {
                logger::warn!(
                    "agentforce-client: startSession got HTTP 401, refreshing token and retrying once"
                );
                let token = self.auth.get_token(client, now_unix, true).await?;
                match self.post_start_session(client, &path, &body, &token).await {
                    Err(ClientError::HttpStatus { status: 401, .. }) => {
                        logger::error!(
                            "agentforce-client: startSession got HTTP 401 after refresh; giving up"
                        );
                        Err(ClientError::AuthRejected)
                    }
                    other => other,
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn post_start_session(
        &self,
        client: &HttpClient,
        path: &str,
        body: &[u8],
        token: &str,
    ) -> Result<StartSessionResponse, ClientError> {
        let bearer = format!("Bearer {token}");
        let response = client
            .request(self.api.as_ref())
            .path(path)
            .headers(vec![
                ("authorization", bearer.as_str()),
                ("content-type", "application/json"),
                ("accept", "application/json"),
            ])
            .body(body)
            .timeout(Duration::from_secs(self.request_timeout_secs))
            .post()
            .await
            .map_err(|e| ClientError::Transport {
                source: anyhow::anyhow!(e.to_string()),
            })?;
        let status = response.status_code();
        if !(200..300).contains(&status) {
            return Err(ClientError::HttpStatus {
                operation: "startSession",
                status,
                body: redact_body(response.body()),
            });
        }
        let parsed: StartSessionResponse = serde_json::from_slice(response.body())
            .map_err(|e| ClientError::BadJson(e.to_string()))?;
        if parsed.session_id.is_empty() {
            return Err(ClientError::MissingField("sessionId"));
        }
        Ok(parsed)
    }

    pub async fn send_message(
        &self,
        client: &HttpClient,
        session_id: &str,
        text: &str,
        sequence_id: u32,
        now_unix: u64,
    ) -> Result<SendMessageResponse, ClientError> {
        let body_struct = SendMessageRequest {
            message: SendMessageBody {
                kind: "Text",
                sequence_id,
                text,
            },
        };
        let body =
            serde_json::to_vec(&body_struct).map_err(|e| ClientError::BadJson(e.to_string()))?;
        let path = self.full_path(&format!(
            "/sessions/{}/messages",
            urlencoding::encode(session_id)
        ));

        let token = self.auth.get_token(client, now_unix, false).await?;
        match self.post_send_message(client, &path, &body, &token).await {
            Ok(r) => Ok(r),
            Err(ClientError::HttpStatus { status: 401, .. }) => {
                logger::warn!(
                    "agentforce-client: sendMessage got HTTP 401, refreshing token and retrying once"
                );
                let token = self.auth.get_token(client, now_unix, true).await?;
                match self.post_send_message(client, &path, &body, &token).await {
                    Err(ClientError::HttpStatus { status: 401, .. }) => {
                        logger::error!(
                            "agentforce-client: sendMessage got HTTP 401 after refresh; giving up"
                        );
                        Err(ClientError::AuthRejected)
                    }
                    other => other,
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn post_send_message(
        &self,
        client: &HttpClient,
        path: &str,
        body: &[u8],
        token: &str,
    ) -> Result<SendMessageResponse, ClientError> {
        let bearer = format!("Bearer {token}");
        let response = client
            .request(self.api.as_ref())
            .path(path)
            .headers(vec![
                ("authorization", bearer.as_str()),
                ("content-type", "application/json"),
                ("accept", "application/json"),
            ])
            .body(body)
            .timeout(Duration::from_secs(self.request_timeout_secs))
            .post()
            .await
            .map_err(|e| ClientError::Transport {
                source: anyhow::anyhow!(e.to_string()),
            })?;
        let status = response.status_code();
        if !(200..300).contains(&status) {
            return Err(ClientError::HttpStatus {
                operation: "sendMessage",
                status,
                body: redact_body(response.body()),
            });
        }
        let parsed: SendMessageResponse = serde_json::from_slice(response.body())
            .map_err(|e| ClientError::BadJson(e.to_string()))?;
        Ok(parsed)
    }

    pub async fn end_session(
        &self,
        client: &HttpClient,
        session_id: &str,
        now_unix: u64,
    ) -> Result<(), ClientError> {
        let path = self.full_path(&format!("/sessions/{}", urlencoding::encode(session_id)));

        let token = self.auth.get_token(client, now_unix, false).await?;
        match self.delete_session(client, &path, &token).await {
            Ok(()) => Ok(()),
            Err(ClientError::HttpStatus { status: 401, .. }) => {
                logger::warn!(
                    "agentforce-client: endSession got HTTP 401, refreshing token and retrying once"
                );
                let token = self.auth.get_token(client, now_unix, true).await?;
                match self.delete_session(client, &path, &token).await {
                    Err(ClientError::HttpStatus { status: 401, .. }) => {
                        logger::error!(
                            "agentforce-client: endSession got HTTP 401 after refresh; giving up"
                        );
                        Err(ClientError::AuthRejected)
                    }
                    other => other,
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn delete_session(
        &self,
        client: &HttpClient,
        path: &str,
        token: &str,
    ) -> Result<(), ClientError> {
        let bearer = format!("Bearer {token}");
        let response = client
            .request(self.api.as_ref())
            .path(path)
            .headers(vec![("authorization", bearer.as_str())])
            .timeout(Duration::from_secs(self.request_timeout_secs))
            .delete()
            .await
            .map_err(|e| ClientError::Transport {
                source: anyhow::anyhow!(e.to_string()),
            })?;
        let status = response.status_code();
        // 200/202/204 = success. 404 = session already gone, treat as success.
        if !(200..300).contains(&status) && status != 404 {
            return Err(ClientError::HttpStatus {
                operation: "endSession",
                status,
                body: redact_body(response.body()),
            });
        }
        Ok(())
    }
}

/// Normalize a base path so it has a single leading `/` and no trailing `/`.
/// Empty string in -> empty string out (signals "no prefix").
pub(crate) fn normalize_base_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return String::new();
    }
    let mut s = trimmed.to_string();
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    s
}

/// Trim very long upstream error bodies for log/error surfaces. Keeps the
/// first 512 chars and notes the original size.
fn redact_body(body: &[u8]) -> String {
    let s = String::from_utf8_lossy(body);
    if s.len() <= 512 {
        s.to_string()
    } else {
        format!("{}...({} bytes)", &s[..512], s.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_session_request_serializes_with_camel_case() {
        let body = StartSessionRequest {
            external_session_key: "abc",
            instance_config: InstanceConfig {
                endpoint: "https://acme.my.salesforce.com",
            },
            feature_support: "Sync",
            streaming_capabilities: StreamingCapabilities::default(),
            bypass_user: true,
        };
        let json: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(json["externalSessionKey"], "abc");
        assert_eq!(json["featureSupport"], "Sync");
        assert_eq!(json["bypassUser"], true);
        assert_eq!(
            json["instanceConfig"]["endpoint"],
            "https://acme.my.salesforce.com"
        );
        assert_eq!(json["streamingCapabilities"]["chunkTypes"][0], "Text");
    }

    #[test]
    fn send_message_request_serializes_with_camel_case() {
        let body = SendMessageRequest {
            message: SendMessageBody {
                kind: "Text",
                sequence_id: 7,
                text: "hi",
            },
        };
        let json: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(json["message"]["type"], "Text");
        assert_eq!(json["message"]["sequenceId"], 7);
        assert_eq!(json["message"]["text"], "hi");
    }

    #[test]
    fn normalize_base_path_handles_common_inputs() {
        assert_eq!(normalize_base_path(""), "");
        assert_eq!(normalize_base_path("/"), "");
        assert_eq!(normalize_base_path("/einstein/ai-agent/v1"), "/einstein/ai-agent/v1");
        assert_eq!(
            normalize_base_path("/einstein/ai-agent/v1/"),
            "/einstein/ai-agent/v1"
        );
        assert_eq!(normalize_base_path("einstein/ai-agent/v1"), "/einstein/ai-agent/v1");
        assert_eq!(normalize_base_path("  /v1  "), "/v1");
    }

    #[test]
    fn redact_body_truncates_long_bodies() {
        let big = "x".repeat(2_000);
        let r = redact_body(big.as_bytes());
        assert!(r.len() < big.len());
        assert!(r.contains("(2000 bytes)"));
    }

    #[test]
    fn parse_start_session_response_minimal() {
        let body = br#"{"sessionId":"abc-123"}"#;
        let r: StartSessionResponse = serde_json::from_slice(body).unwrap();
        assert_eq!(r.session_id, "abc-123");
        assert!(r.messages.is_empty());
    }

    #[test]
    fn parse_send_message_response_with_inform() {
        let body = br#"{"messages":[{"id":"m1","type":"Inform","message":"hello"}]}"#;
        let r: SendMessageResponse = serde_json::from_slice(body).unwrap();
        assert_eq!(r.messages.len(), 1);
        assert_eq!(r.messages[0].id.as_deref(), Some("m1"));
        assert_eq!(r.messages[0].kind.as_deref(), Some("Inform"));
        assert_eq!(r.messages[0].message.as_deref(), Some("hello"));
    }
}
