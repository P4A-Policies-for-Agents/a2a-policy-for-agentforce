//! Typed view over the policy configuration.
//!
//! `pdk build` regenerates `src/generated/config.rs` from `gcl.yaml`. This
//! module wraps the generated struct in a normalized form (defaults applied,
//! enums parsed, agent-card discriminator validated) so the rest of the
//! crate works with a clean strongly-typed `PolicyConfig`.

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("unsupported protocolVersion: {0} (expected 0.3.0)")]
    UnsupportedProtocolVersion(String),

    #[error("unsupported agentCardSource: {0}")]
    UnsupportedAgentCardSource(String),

    #[error("publicBaseUrl is required")]
    MissingPublicBaseUrl,

    #[error("consumerKey is required")]
    MissingConsumerKey,

    #[error("consumerSecret is required")]
    MissingConsumerSecret,

    #[error("agentId is required")]
    MissingAgentId,

    #[error(
        "agentCardSource is 'inline_json' or 'file' but agentCardJson is empty"
    )]
    AgentCardJsonEmpty,

    #[error("agentCardSource is 'url' but no agentCardUrl Service is registered")]
    AgentCardUrlNotRegistered,

    #[error("agentCardSkills entry is missing required field '{0}'")]
    AgentCardSkillEmptyField(&'static str),

    #[error(
        "agentCardSecuritySchemesJson must be a JSON object"
    )]
    AgentCardSecuritySchemesNotObject,

    #[error("agentCardOverrideJson must be a JSON object")]
    AgentCardOverrideNotObject,

    #[error("agentCardJson must be a JSON object")]
    AgentCardJsonNotObject,

    #[error(
        "agentCardProviderOrganization and agentCardProviderUrl must be set together (A2A 0.3.0 §5.5 requires AgentProvider.organization and AgentProvider.url) — leave both blank to omit the provider block"
    )]
    AgentCardProviderHalfSet,

    #[error("invalid JSON in {field}: {error}")]
    InvalidJson { field: &'static str, error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolVersion {
    V0_3_0,
}

impl ProtocolVersion {
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        match s {
            "0.3.0" => Ok(Self::V0_3_0),
            other => Err(ConfigError::UnsupportedProtocolVersion(other.to_string())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::V0_3_0 => "0.3.0",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCardSource {
    InlineJson,
    Url,
    Structured,
    File,
}

impl AgentCardSource {
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        Ok(match s {
            "inline_json" => Self::InlineJson,
            "url" => Self::Url,
            "structured" => Self::Structured,
            "file" => Self::File,
            other => return Err(ConfigError::UnsupportedAgentCardSource(other.to_string())),
        })
    }
}

/// Resolved Exchange-publish settings. Only present when
/// `publishAgentCardToExchange = true` AND the required fields are set.
/// If the toggle is on but credentials/asset id are missing, this stays
/// `None` and the policy logs a warn at first request — never blocks
/// policy load.
#[derive(Debug, Clone)]
pub struct ExchangePublishConfig {
    pub anypoint_client_id: String,
    pub anypoint_client_secret: String,
    pub anypoint_org_id: String,
    pub group_id: String,
    pub asset_id: String,
}

/// One row of the `agentCardSkills` array as supplied by the operator.
/// Decoupled from the codegen-generated struct so unit tests don't need
/// the PDK runtime.
#[derive(Debug, Clone)]
pub struct SkillInput {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
}

/// Structured (form-field) snapshot of the AgentCard. Empty/None values are
/// not added to the resulting JSON object so they don't override good
/// defaults coming from a deep-merge.
#[derive(Debug, Clone, Default)]
pub struct StructuredCardConfig {
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub icon_url: Option<String>,
    pub documentation_url: Option<String>,
    pub provider_organization: Option<String>,
    pub provider_url: Option<String>,
    pub capabilities_streaming: bool,
    pub capabilities_push_notifications: bool,
    pub default_input_modes: Vec<String>,
    pub default_output_modes: Vec<String>,
    pub skills: serde_json::Value,
    pub security_schemes: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct PolicyConfig {
    // Agentforce
    pub consumer_key: String,
    pub consumer_secret: String,
    pub agentforce_access_token_override: Option<String>,
    pub agent_id: String,
    pub bypass_user: bool,
    pub cache_safety_margin_seconds: u32,
    /// Per-call HTTP timeout (seconds) for outbound Agentforce REST calls.
    /// Clamped to 1..=290, default 180. Overrides the PDK client's 10s default.
    pub agentforce_request_timeout_seconds: u32,

    // A2A
    pub protocol_version: ProtocolVersion,
    pub a2a_rpc_path: String,
    pub public_base_url: String,
    pub strict_mode: bool,

    // Task cache (per-replica, in-memory)
    pub task_hot_cache_ttl_seconds: u32,

    // Agent card
    pub agent_card_source: AgentCardSource,
    pub agent_card_json: Option<String>,
    pub structured_card: StructuredCardConfig,
    pub agent_card_override: Option<serde_json::Value>,

    // Exchange publish: Some when toggle is on AND required fields are set.
    pub exchange_publish: Option<ExchangePublishConfig>,

    // Diagnostic flags. Used to A/B test the connected-mode WASM trap
    // hypothesis. Both default to false in production.
    pub diagnostic_pre_body_probe: bool,
    pub diagnostic_pre_body_agentforce_probe: bool,
    pub diagnostic_continue_flow: bool,
}

/// Raw, untyped view of the configuration. The generated codegen produces a
/// near-identical struct with extra Service fields; we narrow to this
/// host-testable shape so all defaulting/validation logic lives in one place
/// and is unit-testable without the full PDK runtime.
#[derive(Debug, Clone, Default)]
pub struct RawConfig {
    pub consumer_key: Option<String>,
    pub consumer_secret: Option<String>,
    pub agentforce_access_token_override: Option<String>,
    pub agent_id: Option<String>,
    pub bypass_user: Option<bool>,
    pub cache_safety_margin_seconds: Option<i64>,
    pub agentforce_request_timeout_seconds: Option<i64>,

    pub protocol_version: Option<String>,
    pub a2a_rpc_path: Option<String>,
    pub public_base_url: Option<String>,
    pub strict_mode: Option<bool>,

    pub task_hot_cache_ttl_seconds: Option<i64>,

    pub agent_card_source: Option<String>,
    pub agent_card_json: Option<String>,
    /// `true` when `agentCardSource = url` and the codegen registered a
    /// Service for the URL. We can't carry the Service through `RawConfig`
    /// (Service is only constructible by the codegen), so the caller passes
    /// this flag in based on whether the URL was supplied.
    pub agent_card_url_registered: bool,
    pub agent_card_name: Option<String>,
    pub agent_card_description: Option<String>,
    pub agent_card_version: Option<String>,
    pub agent_card_icon_url: Option<String>,
    pub agent_card_documentation_url: Option<String>,
    pub agent_card_provider_organization: Option<String>,
    pub agent_card_provider_url: Option<String>,
    pub agent_card_capabilities_streaming: Option<bool>,
    pub agent_card_capabilities_push_notifications: Option<bool>,
    pub agent_card_default_input_modes: Option<String>,
    pub agent_card_default_output_modes: Option<String>,
    pub agent_card_skills: Vec<SkillInput>,
    pub agent_card_security_schemes_json: Option<String>,
    pub agent_card_override_json: Option<String>,

    pub publish_agent_card_to_exchange: Option<bool>,
    pub anypoint_client_id: Option<String>,
    pub anypoint_client_secret: Option<String>,
    pub anypoint_org_id: Option<String>,
    pub exchange_asset_group_id: Option<String>,
    pub exchange_asset_id: Option<String>,

    pub diagnostic_pre_body_probe: Option<bool>,
    pub diagnostic_pre_body_agentforce_probe: Option<bool>,
    pub diagnostic_continue_flow: Option<bool>,
}

impl PolicyConfig {
    pub fn from_raw(raw: RawConfig) -> Result<Self, ConfigError> {
        let consumer_key = nonempty(raw.consumer_key.clone(), ConfigError::MissingConsumerKey)?;
        let consumer_secret = nonempty(
            raw.consumer_secret.clone(),
            ConfigError::MissingConsumerSecret,
        )?;
        let agentforce_access_token_override = raw
            .agentforce_access_token_override
            .clone()
            .filter(|s| !s.trim().is_empty());
        let agent_id = nonempty(raw.agent_id.clone(), ConfigError::MissingAgentId)?;
        let public_base_url =
            nonempty(raw.public_base_url.clone(), ConfigError::MissingPublicBaseUrl)?;

        let protocol_version =
            ProtocolVersion::parse(raw.protocol_version.as_deref().unwrap_or("0.3.0"))?;
        let agent_card_source =
            AgentCardSource::parse(raw.agent_card_source.as_deref().unwrap_or("structured"))?;

        let a2a_rpc_path = normalize_path(raw.a2a_rpc_path.as_deref().unwrap_or("/"));
        let strict_mode = raw.strict_mode.unwrap_or(false);
        let bypass_user = raw.bypass_user.unwrap_or(true);
        let cache_safety_margin_seconds = clamp_u32(raw.cache_safety_margin_seconds, 0, 600, 60);
        let agentforce_request_timeout_seconds =
            clamp_u32(raw.agentforce_request_timeout_seconds, 1, 290, 180);
        let task_hot_cache_ttl_seconds = clamp_u32(raw.task_hot_cache_ttl_seconds, 0, 3600, 60);

        // Validate the source-specific inputs and parse the structured card
        // payload regardless of source so a misconfigured field surfaces at
        // policy load rather than on first request.
        let agent_card_json = match agent_card_source {
            AgentCardSource::InlineJson | AgentCardSource::File => {
                let s = raw
                    .agent_card_json
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or(ConfigError::AgentCardJsonEmpty)?;
                let parsed: serde_json::Value =
                    serde_json::from_str(&s).map_err(|e| ConfigError::InvalidJson {
                        field: "agentCardJson",
                        error: e.to_string(),
                    })?;
                if !parsed.is_object() {
                    return Err(ConfigError::AgentCardJsonNotObject);
                }
                Some(s)
            }
            AgentCardSource::Url => {
                if !raw.agent_card_url_registered {
                    return Err(ConfigError::AgentCardUrlNotRegistered);
                }
                None
            }
            AgentCardSource::Structured => None,
        };

        let structured_card = parse_structured(&raw)?;
        let agent_card_override = parse_optional_object(
            raw.agent_card_override_json.as_deref(),
            "agentCardOverrideJson",
            ConfigError::AgentCardOverrideNotObject,
        )?;

        // Best-effort resolution of the Exchange-publish settings. If the
        // operator turned on the toggle but didn't fill in the required
        // fields, we log at warn (in lib.rs) and skip the publish — never
        // refuse policy load. Refusing would fault every request.
        let exchange_publish = if raw.publish_agent_card_to_exchange.unwrap_or(false) {
            let cid = nonempty_opt(raw.anypoint_client_id.clone());
            let csec = nonempty_opt(raw.anypoint_client_secret.clone());
            let org = nonempty_opt(raw.anypoint_org_id.clone());
            let asset_id = nonempty_opt(raw.exchange_asset_id.clone());
            match (cid, csec, org, asset_id) {
                (Some(cid), Some(csec), Some(org), Some(asset_id)) => {
                    let group_id = nonempty_opt(raw.exchange_asset_group_id.clone())
                        .unwrap_or_else(|| org.clone());
                    Some(ExchangePublishConfig {
                        anypoint_client_id: cid,
                        anypoint_client_secret: csec,
                        anypoint_org_id: org,
                        group_id,
                        asset_id,
                    })
                }
                _ => None,
            }
        } else {
            None
        };

        Ok(Self {
            consumer_key,
            consumer_secret,
            agentforce_access_token_override,
            agent_id,
            bypass_user,
            cache_safety_margin_seconds,
            agentforce_request_timeout_seconds,

            protocol_version,
            a2a_rpc_path,
            public_base_url,
            strict_mode,

            task_hot_cache_ttl_seconds,

            agent_card_source,
            agent_card_json,
            structured_card,
            agent_card_override,

            exchange_publish,

            diagnostic_pre_body_probe: raw.diagnostic_pre_body_probe.unwrap_or(false),
            diagnostic_pre_body_agentforce_probe: raw
                .diagnostic_pre_body_agentforce_probe
                .unwrap_or(false),
            diagnostic_continue_flow: raw.diagnostic_continue_flow.unwrap_or(false),
        })
    }
}

fn parse_structured(raw: &RawConfig) -> Result<StructuredCardConfig, ConfigError> {
    let mut skill_values: Vec<serde_json::Value> = Vec::with_capacity(raw.agent_card_skills.len());
    for skill in &raw.agent_card_skills {
        if skill.id.trim().is_empty() {
            return Err(ConfigError::AgentCardSkillEmptyField("id"));
        }
        if skill.name.trim().is_empty() {
            return Err(ConfigError::AgentCardSkillEmptyField("name"));
        }
        if skill.description.trim().is_empty() {
            return Err(ConfigError::AgentCardSkillEmptyField("description"));
        }
        skill_values.push(serde_json::json!({
            "id": skill.id,
            "name": skill.name,
            "description": skill.description,
            "tags": skill.tags,
        }));
    }
    let skills = serde_json::Value::Array(skill_values);

    let security_schemes = parse_optional_object(
        raw.agent_card_security_schemes_json.as_deref(),
        "agentCardSecuritySchemesJson",
        ConfigError::AgentCardSecuritySchemesNotObject,
    )?;

    let provider_organization = nonempty_opt(raw.agent_card_provider_organization.clone());
    let provider_url = nonempty_opt(raw.agent_card_provider_url.clone());
    if provider_organization.is_some() != provider_url.is_some() {
        return Err(ConfigError::AgentCardProviderHalfSet);
    }

    Ok(StructuredCardConfig {
        name: nonempty_opt(raw.agent_card_name.clone()),
        description: nonempty_opt(raw.agent_card_description.clone()),
        version: nonempty_opt(raw.agent_card_version.clone()),
        icon_url: nonempty_opt(raw.agent_card_icon_url.clone()),
        documentation_url: nonempty_opt(raw.agent_card_documentation_url.clone()),
        provider_organization,
        provider_url,
        capabilities_streaming: raw.agent_card_capabilities_streaming.unwrap_or(false),
        capabilities_push_notifications: raw
            .agent_card_capabilities_push_notifications
            .unwrap_or(false),
        default_input_modes: split_csv(raw.agent_card_default_input_modes.as_deref()),
        default_output_modes: split_csv(raw.agent_card_default_output_modes.as_deref()),
        skills,
        security_schemes,
    })
}

fn parse_optional_object(
    s: Option<&str>,
    field: &'static str,
    not_object: ConfigError,
) -> Result<Option<serde_json::Value>, ConfigError> {
    let s = match s.map(str::trim) {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(None),
    };
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|e| ConfigError::InvalidJson {
            field,
            error: e.to_string(),
        })?;
    if !v.is_object() {
        return Err(not_object);
    }
    Ok(Some(v))
}

fn nonempty(value: Option<String>, err: ConfigError) -> Result<String, ConfigError> {
    match value {
        Some(s) if !s.trim().is_empty() => Ok(s),
        _ => Err(err),
    }
}

fn nonempty_opt(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

fn split_csv(s: Option<&str>) -> Vec<String> {
    let s = s.unwrap_or("text/plain");
    let modes: Vec<String> = s
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    if modes.is_empty() {
        vec!["text/plain".into()]
    } else {
        modes
    }
}

fn normalize_path(s: &str) -> String {
    let mut s = s.trim().to_string();
    if s.is_empty() {
        return "/".into();
    }
    if !s.starts_with('/') {
        s = format!("/{s}");
    }
    s
}

fn clamp_u32(v: Option<i64>, min: i64, max: i64, default: i64) -> u32 {
    let v = v.unwrap_or(default).clamp(min, max);
    v as u32
}

/// Deep-merge `overlay` into `base` in place, recursing into nested objects.
/// Arrays and scalars are replaced wholesale, matching the JSON Merge Patch
/// (RFC 7396) intuition that operators expect when overriding individual
/// AgentCard fields.
pub fn deep_merge(base: &mut serde_json::Value, overlay: serde_json::Value) {
    use serde_json::Value;
    match (base, overlay) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(existing) => deep_merge(existing, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (slot, overlay) => *slot = overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> RawConfig {
        RawConfig {
            consumer_key: Some("client-id".into()),
            consumer_secret: Some("client-secret".into()),
            agent_id: Some("0XXxx0000000000".into()),
            public_base_url: Some("https://gw.example.com/a2a".into()),
            agent_card_source: Some("structured".into()),
            agent_card_name: Some("Test Agent".into()),
            agent_card_description: Some("desc".into()),
            ..Default::default()
        }
    }

    #[test]
    fn applies_defaults() {
        let cfg = PolicyConfig::from_raw(minimal()).unwrap();
        assert_eq!(cfg.protocol_version, ProtocolVersion::V0_3_0);
        assert_eq!(cfg.a2a_rpc_path, "/");
        assert!(!cfg.strict_mode);
        assert!(cfg.bypass_user);
        assert_eq!(cfg.cache_safety_margin_seconds, 60);
        assert_eq!(cfg.agentforce_request_timeout_seconds, 180);
        assert_eq!(cfg.task_hot_cache_ttl_seconds, 60);
        assert_eq!(cfg.agent_card_source, AgentCardSource::Structured);
        assert_eq!(cfg.structured_card.default_input_modes, vec!["text/plain"]);
    }

    #[test]
    fn clamps_agentforce_request_timeout() {
        // Default when unset.
        let cfg = PolicyConfig::from_raw(minimal()).unwrap();
        assert_eq!(cfg.agentforce_request_timeout_seconds, 180);

        // Below the floor (1s).
        let mut raw = minimal();
        raw.agentforce_request_timeout_seconds = Some(0);
        assert_eq!(
            PolicyConfig::from_raw(raw)
                .unwrap()
                .agentforce_request_timeout_seconds,
            1
        );

        // Above the ceiling (290s).
        let mut raw = minimal();
        raw.agentforce_request_timeout_seconds = Some(9999);
        assert_eq!(
            PolicyConfig::from_raw(raw)
                .unwrap()
                .agentforce_request_timeout_seconds,
            290
        );

        // In-range passthrough.
        let mut raw = minimal();
        raw.agentforce_request_timeout_seconds = Some(240);
        assert_eq!(
            PolicyConfig::from_raw(raw)
                .unwrap()
                .agentforce_request_timeout_seconds,
            240
        );
    }

    #[test]
    fn rejects_unknown_protocol_version() {
        let mut raw = minimal();
        raw.protocol_version = Some("9.9.9".into());
        assert!(matches!(
            PolicyConfig::from_raw(raw),
            Err(ConfigError::UnsupportedProtocolVersion(_))
        ));
    }

    #[test]
    fn rejects_unknown_agent_card_source() {
        let mut raw = minimal();
        raw.agent_card_source = Some("smoke-signal".into());
        assert!(matches!(
            PolicyConfig::from_raw(raw),
            Err(ConfigError::UnsupportedAgentCardSource(_))
        ));
    }

    #[test]
    fn rejects_inline_json_when_blank() {
        let mut raw = minimal();
        raw.agent_card_source = Some("inline_json".into());
        raw.agent_card_json = Some("   ".into());
        assert!(matches!(
            PolicyConfig::from_raw(raw),
            Err(ConfigError::AgentCardJsonEmpty)
        ));
    }

    #[test]
    fn rejects_inline_json_when_not_object() {
        let mut raw = minimal();
        raw.agent_card_source = Some("inline_json".into());
        raw.agent_card_json = Some("[]".into());
        assert!(matches!(
            PolicyConfig::from_raw(raw),
            Err(ConfigError::AgentCardJsonNotObject)
        ));
    }

    #[test]
    fn rejects_url_when_no_service_registered() {
        let mut raw = minimal();
        raw.agent_card_source = Some("url".into());
        raw.agent_card_url_registered = false;
        assert!(matches!(
            PolicyConfig::from_raw(raw),
            Err(ConfigError::AgentCardUrlNotRegistered)
        ));
    }

    #[test]
    fn accepts_url_when_service_registered() {
        let mut raw = minimal();
        raw.agent_card_source = Some("url".into());
        raw.agent_card_url_registered = true;
        let cfg = PolicyConfig::from_raw(raw).unwrap();
        assert_eq!(cfg.agent_card_source, AgentCardSource::Url);
        assert!(cfg.agent_card_json.is_none());
    }

    #[test]
    fn rejects_skill_with_blank_id() {
        let mut raw = minimal();
        raw.agent_card_skills = vec![SkillInput {
            id: "  ".into(),
            name: "Greet".into(),
            description: "d".into(),
            tags: vec!["t".into()],
        }];
        assert!(matches!(
            PolicyConfig::from_raw(raw),
            Err(ConfigError::AgentCardSkillEmptyField("id"))
        ));
    }

    #[test]
    fn rejects_skill_with_blank_name() {
        let mut raw = minimal();
        raw.agent_card_skills = vec![SkillInput {
            id: "s1".into(),
            name: "".into(),
            description: "d".into(),
            tags: vec![],
        }];
        assert!(matches!(
            PolicyConfig::from_raw(raw),
            Err(ConfigError::AgentCardSkillEmptyField("name"))
        ));
    }

    #[test]
    fn parses_structured_capabilities_and_modes() {
        let mut raw = minimal();
        raw.agent_card_capabilities_streaming = Some(true);
        raw.agent_card_default_input_modes = Some("text/plain, application/json".into());
        raw.agent_card_skills = vec![SkillInput {
            id: "s1".into(),
            name: "Greet".into(),
            description: "d".into(),
            tags: vec!["t".into()],
        }];
        let cfg = PolicyConfig::from_raw(raw).unwrap();
        assert!(cfg.structured_card.capabilities_streaming);
        assert_eq!(
            cfg.structured_card.default_input_modes,
            vec!["text/plain".to_string(), "application/json".to_string()]
        );
        assert!(cfg.structured_card.skills.is_array());
        assert_eq!(cfg.structured_card.skills[0]["id"], "s1");
        assert_eq!(cfg.structured_card.skills[0]["tags"][0], "t");
    }

    #[test]
    fn provider_org_without_url_is_rejected() {
        let mut raw = minimal();
        raw.agent_card_provider_organization = Some("Acme".into());
        raw.agent_card_provider_url = None;
        assert!(matches!(
            PolicyConfig::from_raw(raw),
            Err(ConfigError::AgentCardProviderHalfSet)
        ));
    }

    #[test]
    fn provider_url_without_org_is_rejected() {
        let mut raw = minimal();
        raw.agent_card_provider_organization = None;
        raw.agent_card_provider_url = Some("https://acme.example.com".into());
        assert!(matches!(
            PolicyConfig::from_raw(raw),
            Err(ConfigError::AgentCardProviderHalfSet)
        ));
    }

    #[test]
    fn provider_both_or_neither_is_accepted() {
        let mut raw = minimal();
        raw.agent_card_provider_organization = Some("Acme".into());
        raw.agent_card_provider_url = Some("https://acme.example.com".into());
        assert!(PolicyConfig::from_raw(raw.clone()).is_ok());

        let mut raw = minimal();
        raw.agent_card_provider_organization = None;
        raw.agent_card_provider_url = None;
        assert!(PolicyConfig::from_raw(raw).is_ok());
    }

    #[test]
    fn override_must_be_object() {
        let mut raw = minimal();
        raw.agent_card_override_json = Some("[1,2,3]".into());
        assert!(matches!(
            PolicyConfig::from_raw(raw),
            Err(ConfigError::AgentCardOverrideNotObject)
        ));
    }

    #[test]
    fn override_when_object_is_kept() {
        let mut raw = minimal();
        raw.agent_card_override_json = Some(r#"{"name":"Override"}"#.into());
        let cfg = PolicyConfig::from_raw(raw).unwrap();
        assert!(cfg.agent_card_override.is_some());
    }

    #[test]
    fn deep_merge_overrides_nested_field() {
        let mut base = serde_json::json!({
            "name": "old",
            "capabilities": { "streaming": false, "pushNotifications": true }
        });
        let overlay = serde_json::json!({
            "capabilities": { "streaming": true }
        });
        deep_merge(&mut base, overlay);
        assert_eq!(base["capabilities"]["streaming"], true);
        assert_eq!(base["capabilities"]["pushNotifications"], true);
        assert_eq!(base["name"], "old");
    }

    #[test]
    fn deep_merge_replaces_arrays_wholesale() {
        let mut base = serde_json::json!({ "tags": ["a", "b"] });
        let overlay = serde_json::json!({ "tags": ["c"] });
        deep_merge(&mut base, overlay);
        assert_eq!(base["tags"], serde_json::json!(["c"]));
    }

    #[test]
    fn normalize_path_adds_leading_slash() {
        assert_eq!(normalize_path("a2a"), "/a2a");
        assert_eq!(normalize_path("/a2a"), "/a2a");
        assert_eq!(normalize_path(""), "/");
    }

    #[test]
    fn clamp_u32_respects_bounds() {
        assert_eq!(clamp_u32(Some(-5), 0, 100, 10), 0);
        assert_eq!(clamp_u32(Some(200), 0, 100, 10), 100);
        assert_eq!(clamp_u32(None, 0, 100, 10), 10);
    }
}
