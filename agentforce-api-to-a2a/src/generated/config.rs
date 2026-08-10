use serde::Deserialize;
#[derive(Deserialize, Clone, Debug)]
pub struct AgentCardSkills0Config {
    #[serde(alias = "description")]
    pub description: String,
    #[serde(alias = "id")]
    pub id: String,
    #[serde(alias = "name")]
    pub name: String,
    #[serde(alias = "tags")]
    pub tags: Option<Vec<String>>,
}
#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(alias = "a2aRpcPath")]
    pub a_2_a_rpc_path: Option<String>,
    #[serde(alias = "agentCardCapabilitiesPushNotifications")]
    pub agent_card_capabilities_push_notifications: Option<bool>,
    #[serde(alias = "agentCardCapabilitiesStreaming")]
    pub agent_card_capabilities_streaming: Option<bool>,
    #[serde(alias = "agentCardDefaultInputModes")]
    pub agent_card_default_input_modes: Option<String>,
    #[serde(alias = "agentCardDefaultOutputModes")]
    pub agent_card_default_output_modes: Option<String>,
    #[serde(alias = "agentCardDescription")]
    pub agent_card_description: Option<String>,
    #[serde(alias = "agentCardDocumentationUrl")]
    pub agent_card_documentation_url: Option<String>,
    #[serde(alias = "agentCardIconUrl")]
    pub agent_card_icon_url: Option<String>,
    #[serde(alias = "agentCardJson")]
    pub agent_card_json: Option<String>,
    #[serde(alias = "agentCardName")]
    pub agent_card_name: Option<String>,
    #[serde(alias = "agentCardOverrideJson")]
    pub agent_card_override_json: Option<String>,
    #[serde(alias = "agentCardProviderOrganization")]
    pub agent_card_provider_organization: Option<String>,
    #[serde(alias = "agentCardProviderUrl")]
    pub agent_card_provider_url: Option<String>,
    #[serde(alias = "agentCardSecuritySchemesJson")]
    pub agent_card_security_schemes_json: Option<String>,
    #[serde(alias = "agentCardSkills")]
    pub agent_card_skills: Option<Vec<AgentCardSkills0Config>>,
    #[serde(alias = "agentCardSource")]
    pub agent_card_source: Option<String>,
    #[serde(
        alias = "agentCardUrl",
        default,
        deserialize_with = "pdk::serde::deserialize_service_opt"
    )]
    pub agent_card_url: Option<pdk::hl::Service>,
    #[serde(alias = "agentCardVersion")]
    pub agent_card_version: Option<String>,
    #[serde(alias = "agentId")]
    pub agent_id: String,
    #[serde(alias = "agentforceAccessTokenOverride")]
    pub agentforce_access_token_override: Option<String>,
    #[serde(alias = "agentforceApiBasePath")]
    pub agentforce_api_base_path: Option<String>,
    #[serde(
        alias = "agentforceApiUrl",
        deserialize_with = "pdk::serde::deserialize_service"
    )]
    pub agentforce_api_url: pdk::hl::Service,
    #[serde(alias = "agentforceRequestTimeoutSeconds")]
    pub agentforce_request_timeout_seconds: Option<i64>,
    #[serde(alias = "anypointClientId")]
    pub anypoint_client_id: Option<String>,
    #[serde(alias = "anypointClientSecret")]
    pub anypoint_client_secret: Option<String>,
    #[serde(alias = "anypointOrgId")]
    pub anypoint_org_id: Option<String>,
    #[serde(
        alias = "anypointTokenUrl",
        default,
        deserialize_with = "pdk::serde::deserialize_service_opt"
    )]
    pub anypoint_token_url: Option<pdk::hl::Service>,
    #[serde(alias = "bypassUser")]
    pub bypass_user: Option<bool>,
    #[serde(alias = "cacheSafetyMarginSeconds")]
    pub cache_safety_margin_seconds: Option<i64>,
    #[serde(alias = "consumerKey")]
    pub consumer_key: String,
    #[serde(alias = "consumerSecret")]
    pub consumer_secret: String,
    #[serde(alias = "diagnosticContinueFlow")]
    pub diagnostic_continue_flow: Option<bool>,
    #[serde(alias = "diagnosticPreBodyAgentforceProbe")]
    pub diagnostic_pre_body_agentforce_probe: Option<bool>,
    #[serde(alias = "diagnosticPreBodyProbe")]
    pub diagnostic_pre_body_probe: Option<bool>,
    #[serde(alias = "exchangeAssetGroupId")]
    pub exchange_asset_group_id: Option<String>,
    #[serde(alias = "exchangeAssetId")]
    pub exchange_asset_id: Option<String>,
    #[serde(
        alias = "exchangeBaseUrl",
        default,
        deserialize_with = "pdk::serde::deserialize_service_opt"
    )]
    pub exchange_base_url: Option<pdk::hl::Service>,
    #[serde(alias = "myDomainUrl", deserialize_with = "pdk::serde::deserialize_service")]
    pub my_domain_url: pdk::hl::Service,
    #[serde(alias = "protocolVersion")]
    pub protocol_version: Option<String>,
    #[serde(alias = "publicBaseUrl")]
    pub public_base_url: String,
    #[serde(alias = "publishAgentCardToExchange")]
    pub publish_agent_card_to_exchange: Option<bool>,
    #[serde(alias = "strictMode")]
    pub strict_mode: Option<bool>,
    #[serde(alias = "taskHotCacheTtlSeconds")]
    pub task_hot_cache_ttl_seconds: Option<i64>,
}
#[pdk::hl::entrypoint_flex]
fn init(abi: &dyn pdk::flex_abi::api::FlexAbi) -> Result<(), anyhow::Error> {
    let config: Config = serde_json::from_slice(abi.get_configuration())
        .map_err(|err| {
            anyhow::anyhow!(
                "Failed to parse configuration '{}'. Cause: {}",
                String::from_utf8_lossy(abi.get_configuration()), err
            )
        })?;
    if config.agent_card_url.is_some() {
        let service = config.agent_card_url.unwrap();
        abi.service_create(service)?;
    }
    abi.service_create(config.agentforce_api_url)?;
    if config.anypoint_token_url.is_some() {
        let service = config.anypoint_token_url.unwrap();
        abi.service_create(service)?;
    }
    if config.exchange_base_url.is_some() {
        let service = config.exchange_base_url.unwrap();
        abi.service_create(service)?;
    }
    abi.service_create(config.my_domain_url)?;
    abi.setup()?;
    Ok(())
}
