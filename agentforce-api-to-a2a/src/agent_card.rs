//! `AgentCard` resolution and serving.
//!
//! Four sources are supported (chosen via `agentCardSource`):
//!
//!   * `inline_json` - operator pastes a complete card into the textarea.
//!   * `file`        - same field; documented as the "paste-the-file"
//!                     workflow because the API Manager UI has no file
//!                     upload widget.
//!   * `structured`  - operator fills in the form fields; we synthesize
//!                     the card from them.
//!   * `url`         - URL registered as a Service; fetched lazily on the
//!                     first `/.well-known/agent-card.json` request and
//!                     memoized in the PDK shared cache.
//!
//! After resolving from the chosen source, `agentCardOverrideJson` is
//! deep-merged on top, then we force three fields to authoritative values:
//! `protocolVersion`, `url`, `preferredTransport = "JSONRPC"`. The card
//! cannot lie about where it is hosted or which protocol it speaks.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use pdk::cache::Cache;
use pdk::hl::{HttpClient, Service};
use pdk::logger;
use serde_json::{json, Value};
use thiserror::Error;

use crate::config::{deep_merge, AgentCardSource, PolicyConfig, StructuredCardConfig};

/// Cache key used to memoize a URL-source AgentCard in the PDK shared
/// cache so cold restarts don't lose the fetched copy if the cache
/// happens to outlive the policy load.
const CACHE_KEY_URL_CARD: &str = "agent-card:url-fetched";

/// How long the URL-fetched card lives in the PDK cache. Long enough
/// that we don't hammer the URL on every request, short enough that an
/// operator who fixes a bad URL sees the new card within minutes
/// without redeploying.
const URL_CACHE_TTL_SECS: u64 = 600;

const URL_FETCH_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Error)]
pub enum CardError {
    #[error("agent card url fetch transport error: {0}")]
    Transport(String),

    #[error("agent card url fetch returned HTTP {0}")]
    HttpStatus(u32),

    #[error("agent card body was not valid JSON: {0}")]
    BadJson(String),

    #[error("agent card body was not a JSON object")]
    NotObject,
}

/// Build the structured card view as a JSON object from the operator's
/// individual form fields. Empty fields are omitted so the deep-merge
/// keeps any sane defaults coming from `agentCardOverrideJson`.
pub fn structured_to_card(s: &StructuredCardConfig) -> Value {
    let mut obj = serde_json::Map::new();

    if let Some(v) = &s.name {
        obj.insert("name".into(), Value::String(v.clone()));
    }
    if let Some(v) = &s.description {
        obj.insert("description".into(), Value::String(v.clone()));
    }
    if let Some(v) = &s.version {
        obj.insert("version".into(), Value::String(v.clone()));
    }
    if let Some(v) = &s.icon_url {
        obj.insert("iconUrl".into(), Value::String(v.clone()));
    }
    if let Some(v) = &s.documentation_url {
        obj.insert("documentationUrl".into(), Value::String(v.clone()));
    }

    // A2A 0.3.0 §5.5 requires AgentProvider to carry both `organization`
    // AND `url`. Emit the provider block only when both fields are
    // present; otherwise drop it entirely so the resulting card is still
    // spec-valid. The config layer rejects "only one of the two".
    if let (Some(org), Some(url)) = (&s.provider_organization, &s.provider_url) {
        obj.insert(
            "provider".into(),
            json!({ "organization": org, "url": url }),
        );
    }

    obj.insert(
        "capabilities".into(),
        json!({
            "streaming": s.capabilities_streaming,
            "pushNotifications": s.capabilities_push_notifications,
        }),
    );

    obj.insert(
        "defaultInputModes".into(),
        Value::Array(s.default_input_modes.iter().cloned().map(Value::String).collect()),
    );
    obj.insert(
        "defaultOutputModes".into(),
        Value::Array(s.default_output_modes.iter().cloned().map(Value::String).collect()),
    );
    obj.insert("skills".into(), s.skills.clone());
    if let Some(ss) = &s.security_schemes {
        obj.insert("securitySchemes".into(), ss.clone());
    }

    Value::Object(obj)
}

/// Apply override + force-set the policy-controlled fields. Returns the
/// final card JSON object.
pub fn finalize_card(
    mut card: Value,
    override_json: Option<&Value>,
    public_base_url: &str,
    a2a_rpc_path: &str,
    protocol_version: &str,
) -> Value {
    if let Some(o) = override_json {
        deep_merge(&mut card, o.clone());
    }

    if let Value::Object(map) = &mut card {
        map.insert(
            "protocolVersion".into(),
            Value::String(protocol_version.to_string()),
        );
        map.insert(
            "url".into(),
            Value::String(join_url(public_base_url, a2a_rpc_path)),
        );
        map.insert(
            "preferredTransport".into(),
            Value::String("JSONRPC".to_string()),
        );
    }

    card
}

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if path == "/" {
        base.to_string()
    } else {
        format!("{base}{path}")
    }
}

/// Build the structured / inline JSON card snapshot. Does not include a
/// URL fetch (which has to be lazy-async at request time).
pub fn build_static_card(cfg: &PolicyConfig) -> Result<Value, CardError> {
    let card = match cfg.agent_card_source {
        AgentCardSource::Structured => structured_to_card(&cfg.structured_card),
        AgentCardSource::InlineJson | AgentCardSource::File => {
            let s = cfg
                .agent_card_json
                .as_deref()
                .expect("validated at config load");
            let v: Value = serde_json::from_str(s).map_err(|e| CardError::BadJson(e.to_string()))?;
            if !v.is_object() {
                return Err(CardError::NotObject);
            }
            v
        }
        AgentCardSource::Url => {
            // URL source produces nothing here; the runtime fetcher will
            // build the card on first access.
            return Ok(Value::Object(serde_json::Map::new()));
        }
    };

    Ok(finalize_card(
        card,
        cfg.agent_card_override.as_ref(),
        &cfg.public_base_url,
        &cfg.a2a_rpc_path,
        cfg.protocol_version.as_str(),
    ))
}

/// Holder for the resolved card. For non-URL sources, `prebuilt_bytes` is
/// populated at policy load and reused for every request. For URL source,
/// we fetch lazily on first request and cache in memory + the PDK shared
/// cache.
pub struct CardProvider {
    cfg: Rc<PolicyConfig>,
    prebuilt_bytes: RefCell<Option<Rc<Vec<u8>>>>,
    cache: Rc<dyn Cache>,
    /// Service registered for `agentCardUrl`. Only `Some` when
    /// `agentCardSource = url`.
    url_service: Option<Rc<Service>>,
}

impl CardProvider {
    pub fn new(
        cfg: Rc<PolicyConfig>,
        cache: Rc<dyn Cache>,
        url_service: Option<Rc<Service>>,
    ) -> Self {
        Self {
            cfg,
            prebuilt_bytes: RefCell::new(None),
            cache,
            url_service,
        }
    }

    /// Eagerly build the static card so a config error surfaces at policy
    /// load rather than on first request. URL sources return Ok with an
    /// empty-bytes signal; the actual fetch happens in `bytes`.
    pub fn warm(&self) -> Result<(), CardError> {
        if matches!(self.cfg.agent_card_source, AgentCardSource::Url) {
            return Ok(());
        }
        let v = build_static_card(&self.cfg)?;
        let bytes = serde_json::to_vec(&v).expect("Value serializes");
        *self.prebuilt_bytes.borrow_mut() = Some(Rc::new(bytes));
        Ok(())
    }

    /// Return the prebuilt card bytes if `warm()` has populated them
    /// (i.e. for non-URL sources). Returns `None` for URL sources whose
    /// card has not yet been fetched.
    pub fn warmed_bytes(&self) -> Option<Rc<Vec<u8>>> {
        self.prebuilt_bytes.borrow().clone()
    }

    /// Return the AgentCard JSON bytes ready to write to the wire.
    pub async fn bytes(&self, http: &HttpClient, now_unix: u64) -> Result<Rc<Vec<u8>>, CardError> {
        if let Some(b) = self.prebuilt_bytes.borrow().as_ref() {
            return Ok(b.clone());
        }
        // URL source: try cache, then fetch.
        if matches!(self.cfg.agent_card_source, AgentCardSource::Url) {
            if let Some(bytes) = self.read_cached_url_card(now_unix) {
                let rc = Rc::new(bytes);
                *self.prebuilt_bytes.borrow_mut() = Some(rc.clone());
                return Ok(rc);
            }
            let svc = self
                .url_service
                .as_ref()
                .ok_or(CardError::NotObject)?
                .clone();
            let raw = fetch_url_card(http, svc.as_ref()).await?;
            let v: Value = serde_json::from_slice(&raw).map_err(|e| CardError::BadJson(e.to_string()))?;
            if !v.is_object() {
                return Err(CardError::NotObject);
            }
            let final_v = finalize_card(
                v,
                self.cfg.agent_card_override.as_ref(),
                &self.cfg.public_base_url,
                &self.cfg.a2a_rpc_path,
                self.cfg.protocol_version.as_str(),
            );
            let bytes = serde_json::to_vec(&final_v).expect("Value serializes");
            self.write_cached_url_card(&bytes, now_unix);
            let rc = Rc::new(bytes);
            *self.prebuilt_bytes.borrow_mut() = Some(rc.clone());
            return Ok(rc);
        }
        // Non-URL source not yet warmed; do it now (defensive).
        let v = build_static_card(&self.cfg)?;
        let bytes = serde_json::to_vec(&v).expect("Value serializes");
        let rc = Rc::new(bytes);
        *self.prebuilt_bytes.borrow_mut() = Some(rc.clone());
        Ok(rc)
    }

    fn read_cached_url_card(&self, now_unix: u64) -> Option<Vec<u8>> {
        let bytes = self.cache.get(CACHE_KEY_URL_CARD)?;
        let env: UrlCardEnvelope = serde_json::from_slice(&bytes).ok()?;
        if env.expires_at_unix <= now_unix {
            self.cache.delete(CACHE_KEY_URL_CARD);
            None
        } else {
            Some(env.payload)
        }
    }

    fn write_cached_url_card(&self, bytes: &[u8], now_unix: u64) {
        let env = UrlCardEnvelope {
            payload: bytes.to_vec(),
            expires_at_unix: now_unix.saturating_add(URL_CACHE_TTL_SECS),
        };
        if let Ok(b) = serde_json::to_vec(&env) {
            let _ = self.cache.save(CACHE_KEY_URL_CARD, b);
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct UrlCardEnvelope {
    payload: Vec<u8>,
    expires_at_unix: u64,
}

async fn fetch_url_card(http: &HttpClient, svc: &Service) -> Result<Vec<u8>, CardError> {
    let response = http
        .request(svc)
        .headers(vec![("accept", "application/json")])
        .timeout(Duration::from_secs(URL_FETCH_TIMEOUT_SECS))
        .get()
        .await
        .map_err(|e| CardError::Transport(e.to_string()))?;
    let status = response.status_code();
    if !(200..300).contains(&status) {
        logger::warn!("agent-card: url fetch returned HTTP {status}");
        return Err(CardError::HttpStatus(status));
    }
    Ok(response.body().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentCardSource, PolicyConfig, ProtocolVersion};

    fn cfg_with(source: AgentCardSource, json: Option<String>) -> PolicyConfig {
        PolicyConfig {
            consumer_key: "k".into(),
            consumer_secret: "s".into(),
            agentforce_access_token_override: None,
            agent_id: "a".into(),
            bypass_user: true,
            cache_safety_margin_seconds: 60,
            agentforce_request_timeout_seconds: 180,
            protocol_version: ProtocolVersion::V0_3_0,
            a2a_rpc_path: "/".into(),
            public_base_url: "https://gw.example.com/a2a".into(),
            strict_mode: false,
            diagnostic_pre_body_probe: false,
            diagnostic_pre_body_agentforce_probe: false,
            diagnostic_continue_flow: false,
            task_hot_cache_ttl_seconds: 60,
            agent_card_source: source,
            agent_card_json: json,
            exchange_publish: None,
            structured_card: StructuredCardConfig {
                name: Some("Hello Agent".into()),
                description: Some("desc".into()),
                version: Some("1.0.0".into()),
                icon_url: None,
                documentation_url: None,
                provider_organization: Some("Acme".into()),
                provider_url: Some("https://acme.example.com".into()),
                capabilities_streaming: false,
                capabilities_push_notifications: false,
                default_input_modes: vec!["text/plain".into()],
                default_output_modes: vec!["text/plain".into()],
                skills: serde_json::json!([
                    {"id":"s1","name":"Greet","description":"d","tags":["t"]}
                ]),
                security_schemes: None,
            },
            agent_card_override: None,
        }
    }

    #[test]
    fn structured_card_includes_capabilities_and_skills() {
        let cfg = cfg_with(AgentCardSource::Structured, None);
        let card = build_static_card(&cfg).unwrap();
        assert_eq!(card["name"], "Hello Agent");
        assert_eq!(card["protocolVersion"], "0.3.0");
        assert_eq!(card["preferredTransport"], "JSONRPC");
        assert_eq!(card["url"], "https://gw.example.com/a2a");
        assert_eq!(card["capabilities"]["streaming"], false);
        assert_eq!(card["skills"][0]["id"], "s1");
        assert_eq!(card["provider"]["organization"], "Acme");
        assert_eq!(card["provider"]["url"], "https://acme.example.com");
    }

    #[test]
    fn structured_card_omits_provider_when_both_blank() {
        let mut cfg = cfg_with(AgentCardSource::Structured, None);
        cfg.structured_card.provider_organization = None;
        cfg.structured_card.provider_url = None;
        let card = build_static_card(&cfg).unwrap();
        assert!(
            card.get("provider").is_none(),
            "provider must be absent when neither org nor url is set, got {card}"
        );
    }

    #[test]
    fn inline_json_overrides_structured_when_chosen() {
        let mut cfg = cfg_with(
            AgentCardSource::InlineJson,
            Some(r#"{"name":"Inline","version":"9.9.9"}"#.into()),
        );
        cfg.structured_card.name = Some("Should be ignored".into());
        let card = build_static_card(&cfg).unwrap();
        assert_eq!(card["name"], "Inline");
        assert_eq!(card["version"], "9.9.9");
        // protocolVersion is forced regardless of inline value.
        assert_eq!(card["protocolVersion"], "0.3.0");
    }

    #[test]
    fn override_json_deep_merges() {
        let mut cfg = cfg_with(AgentCardSource::Structured, None);
        cfg.agent_card_override = Some(serde_json::json!({
            "capabilities": { "streaming": true },
            "extra": "carried"
        }));
        let card = build_static_card(&cfg).unwrap();
        assert_eq!(card["capabilities"]["streaming"], true);
        assert_eq!(card["capabilities"]["pushNotifications"], false);
        assert_eq!(card["extra"], "carried");
    }

    #[test]
    fn forced_url_appends_path_when_present() {
        let mut cfg = cfg_with(AgentCardSource::Structured, None);
        cfg.public_base_url = "https://gw.example.com/a2a".into();
        cfg.a2a_rpc_path = "/rpc".into();
        let card = build_static_card(&cfg).unwrap();
        assert_eq!(card["url"], "https://gw.example.com/a2a/rpc");
    }

    #[test]
    fn forced_url_keeps_base_when_path_is_root() {
        let mut cfg = cfg_with(AgentCardSource::Structured, None);
        cfg.public_base_url = "https://gw.example.com/a2a/".into();
        cfg.a2a_rpc_path = "/".into();
        let card = build_static_card(&cfg).unwrap();
        assert_eq!(card["url"], "https://gw.example.com/a2a");
    }

    #[test]
    fn url_source_yields_empty_static_card() {
        let cfg = cfg_with(AgentCardSource::Url, None);
        let card = build_static_card(&cfg).unwrap();
        assert!(card.as_object().unwrap().is_empty());
    }
}
