//! Agentforce API -> A2A 0.3.0 server policy.
//!
//! Two-phase architecture:
//!
//! On request (`on_request`):
//!   1. Classify the path (`agent-card`, `a2a-rpc`, or passthrough).
//!   2. agent-card: short-circuit with the cached/fetched AgentCard JSON.
//!   3. a2a-rpc: read the body, parse JSON-RPC.
//!      - For `message/send` (which needs outbound calls to Salesforce):
//!        record the parsed request as `A2APending::MessageSend` and
//!        return `Flow::Continue` so the request flows to upstream. The
//!        actual outbound calls are deferred to the response phase.
//!      - For methods that do not require outbound (e.g. `tasks/get` /
//!        `tasks/cancel` when the task is missing or OS2 is disabled):
//!        dispatch synchronously and short-circuit with the JSON-RPC
//!        response.
//!      - For unknown methods or malformed bodies: short-circuit with
//!        the appropriate JSON-RPC error.
//!   4. passthrough: continue (or 404 if `strictMode = true`).
//!
//! On response (`on_response`):
//!   - If the request phase deferred a `message/send`, re-parse the
//!     JSON-RPC body, dispatch (this is where outbound HTTPS calls to
//!     Salesforce/Agentforce happen), and replace the upstream's
//!     response with the policy-built A2A `Task` JSON via
//!     `ResponseHeadersState::send_response`.
//!   - Otherwise, let the upstream's response pass through unchanged.
//!
//! The reason for the two-phase split is a connected-mode Flex Gateway
//! runtime quirk: outbound HTTPS issued from a WASM filter after
//! `request.into_body_state().await` traps the filter (returning an
//! empty-body 404 to the client). The same outbound issued before the
//! body-state transition, or from the response phase, works fine. By
//! moving the multi-step Agentforce orchestration to `on_response` we
//! avoid issuing any outbound from the body-state phase of the request.

mod a2a;
mod agent_card;
mod agentforce;
mod cache;
mod config;
mod exchange;
mod generated;
mod jsonrpc;
mod router;
mod store;

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::anyhow;
use pdk::cache::{Cache, CacheBuilder};
use pdk::hl::*;
use pdk::logger;

use crate::a2a::methods::Dispatcher;
use crate::agent_card::CardProvider;
use crate::agentforce::auth::{AgentforceAuth, AgentforceAuthConfig};
use crate::agentforce::client::AgentforceClient;
use crate::config::{PolicyConfig, RawConfig};
use crate::generated::config::Config;
use crate::jsonrpc::{JsonRpcError, INVALID_REQUEST, METHOD_NOT_FOUND};
use crate::router::{classify, Route};
use crate::store::task_store::TaskStore;

/// Cache id for the worker-shared PDK cache. Stores the Salesforce
/// OAuth token, the URL-fetched AgentCard, and the in-memory TaskStore.
const CACHE_ID: &str = "agentforce-a2a-shared";

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// ISO-8601 UTC timestamp for the current moment. Best-effort: if the
/// host clock is broken we still produce something serializable so we
/// never fail an RPC over a missing timestamp.
fn now_iso() -> String {
    let secs = now_unix();
    let (year, month, day, hour, min, sec) = unix_to_ymdhms(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Tiny, dependency-free UNIX-seconds -> Y/M/D/h/m/s converter (UTC).
/// Avoids pulling `chrono` into the wasm binary.
fn unix_to_ymdhms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let day_secs = 86_400;
    let mut days = (secs / day_secs) as i64;
    let secs_of_day = (secs % day_secs) as u32;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;

    let mut year = 1970i64;
    loop {
        let leap = is_leap(year);
        let dy = if leap { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let mdays = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 0;
    for (i, m) in mdays.iter().enumerate() {
        if days < *m {
            month = i + 1;
            break;
        }
        days -= *m;
    }
    let day = days as u32 + 1;
    (year as u32, month as u32, day, hour, min, sec)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

/// Bridge codegen-typed `Config` into the host-testable `RawConfig`.
/// Service-typed properties (`myDomainUrl`, etc.) stay on `Config` and
/// are consumed directly in `configure`.
impl From<&Config> for RawConfig {
    fn from(c: &Config) -> Self {
        RawConfig {
            consumer_key: Some(c.consumer_key.clone()),
            consumer_secret: Some(c.consumer_secret.clone()),
            agentforce_access_token_override: c.agentforce_access_token_override.clone(),
            agent_id: Some(c.agent_id.clone()),
            bypass_user: c.bypass_user,
            cache_safety_margin_seconds: c.cache_safety_margin_seconds,
            agentforce_request_timeout_seconds: c.agentforce_request_timeout_seconds,
            protocol_version: c.protocol_version.clone(),
            a2a_rpc_path: c.a_2_a_rpc_path.clone(),
            public_base_url: Some(c.public_base_url.clone()),
            strict_mode: c.strict_mode,
            task_hot_cache_ttl_seconds: c.task_hot_cache_ttl_seconds,
            agent_card_source: c.agent_card_source.clone(),
            agent_card_json: c.agent_card_json.clone(),
            agent_card_url_registered: c.agent_card_url.is_some(),
            agent_card_name: c.agent_card_name.clone(),
            agent_card_description: c.agent_card_description.clone(),
            agent_card_version: c.agent_card_version.clone(),
            agent_card_icon_url: c.agent_card_icon_url.clone(),
            agent_card_documentation_url: c.agent_card_documentation_url.clone(),
            agent_card_provider_organization: c.agent_card_provider_organization.clone(),
            agent_card_provider_url: c.agent_card_provider_url.clone(),
            agent_card_capabilities_streaming: c.agent_card_capabilities_streaming,
            agent_card_capabilities_push_notifications: c.agent_card_capabilities_push_notifications,
            agent_card_default_input_modes: c.agent_card_default_input_modes.clone(),
            agent_card_default_output_modes: c.agent_card_default_output_modes.clone(),
            agent_card_skills: c
                .agent_card_skills
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|s| crate::config::SkillInput {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    description: s.description.clone(),
                    tags: s.tags.clone().unwrap_or_default(),
                })
                .collect(),
            agent_card_security_schemes_json: c.agent_card_security_schemes_json.clone(),
            agent_card_override_json: c.agent_card_override_json.clone(),

            publish_agent_card_to_exchange: c.publish_agent_card_to_exchange,
            anypoint_client_id: c.anypoint_client_id.clone(),
            anypoint_client_secret: c.anypoint_client_secret.clone(),
            anypoint_org_id: c.anypoint_org_id.clone(),
            exchange_asset_group_id: c.exchange_asset_group_id.clone(),
            exchange_asset_id: c.exchange_asset_id.clone(),

            diagnostic_pre_body_probe: c.diagnostic_pre_body_probe,
            diagnostic_pre_body_agentforce_probe: c.diagnostic_pre_body_agentforce_probe,
            diagnostic_continue_flow: c.diagnostic_continue_flow,
        }
    }
}

#[derive(Clone)]
struct PolicyState {
    cfg: Rc<PolicyConfig>,
    card: Rc<CardProvider>,
    dispatcher: Rc<Dispatcher>,
    /// Held only so the diagnostic pre-body probe can call
    /// `auth.get_token` from the `RequestHeaders` state. Outside of the
    /// diagnostic path, `auth` is owned by `dispatcher` already.
    auth: Rc<AgentforceAuth>,

    /// Shared cache used by the Exchange-publish OAuth path.
    cache: Rc<dyn Cache>,
    /// Service for the Anypoint OAuth token endpoint. Only present when
    /// codegen registered a Service (i.e. operator left the default).
    anypoint_token_service: Option<Rc<Service>>,
    /// Service for `exchangeBaseUrl`. Same caveat as above.
    exchange_service: Option<Rc<Service>>,
    /// Authority of the token URL (used as cache-key salt).
    anypoint_token_authority: String,
    /// Fire-once flag: the publish runs at most once per replica, on the
    /// first inbound request after `configure()`. Per-replica, in-memory.
    publish_done: Rc<std::cell::Cell<bool>>,
}

/// User data carried from the request phase to the response phase.
#[derive(Clone, Debug)]
enum A2APending {
    /// Pass-through (non-A2A request, or strict-mode-allowed); response
    /// phase is a no-op and the upstream's response flows through
    /// unchanged.
    PassThrough,
    /// `message/send` was deferred to the response phase. Carry the
    /// parsed JSON-RPC id and the raw body so the response handler can
    /// re-parse and dispatch.
    MessageSend {
        id: Option<serde_json::Value>,
        body: Vec<u8>,
        now_unix: u64,
        now_iso: String,
    },
}

async fn request_filter(
    request: RequestHeadersState,
    state: PolicyState,
    client: HttpClient,
) -> Flow<A2APending> {
    let method = request.method();
    let path = request.path();
    let route = classify(&method, &path, &state.cfg.a2a_rpc_path);
    logger::debug!(
        "a2a: classify method='{}' path='{}' a2a_rpc_path='{}' -> {:?}",
        method,
        path,
        state.cfg.a2a_rpc_path,
        route
    );

    // Fire-once Exchange asset publish on the first request to land on
    // each replica. Outbound HTTP is safe in this context (request-headers
    // phase, before the body-state transition). Best-effort: any failure
    // logs at warn and the request continues normally.
    maybe_publish_agent_card(&state, &client).await;

    match route {
        Route::AgentCard => match state.card.bytes(&client, now_unix()).await {
            Ok(bytes) => Flow::Break(make_json_response(200, bytes.as_slice())),
            Err(e) => {
                logger::error!("agent-card: failed to provide card: {e}");
                Flow::Break(make_json_response(
                    500,
                    br#"{"error":"agent_card_unavailable"}"#,
                ))
            }
        },
        Route::A2aRpc => handle_rpc_request(request, state, client).await,
        Route::Passthrough => {
            if state.cfg.strict_mode {
                Flow::Break(make_json_response(
                    404,
                    br#"{"error":"not_found","reason":"strictMode"}"#,
                ))
            } else {
                Flow::Continue(A2APending::PassThrough)
            }
        }
    }
}

async fn handle_rpc_request(
    request: RequestHeadersState,
    state: PolicyState,
    client: HttpClient,
) -> Flow<A2APending> {
    logger::error!("a2a-trace: handle_rpc enter");
    if !request.contains_body() {
        logger::error!("a2a-trace: handle_rpc no body");
        let err = JsonRpcError::new(None, INVALID_REQUEST, "Invalid Request: empty body");
        return Flow::Break(make_json_response(200, err.into_bytes().as_slice()));
    }

    logger::error!("a2a-trace: handle_rpc reading body");
    let body_state = request.into_body_state().await;
    let body = body_state.handler().body();
    logger::error!("a2a-trace: handle_rpc body read, len={}", body.len());

    let parsed = match jsonrpc::parse_request(&body) {
        Ok(r) => r,
        Err(err) => {
            logger::error!("a2a-trace: handle_rpc parse failed");
            return Flow::Break(make_json_response(200, err.into_bytes().as_slice()));
        }
    };
    logger::error!("a2a-trace: handle_rpc parsed method='{}'", parsed.method);

    match parsed.method.as_str() {
        "message/send" => {
            // Connected-mode Flex Gateway traps WASM outbound HTTPS made
            // after the request body has been read. Defer the actual
            // dispatch (and its outbound calls) to the response phase by
            // returning Flow::Continue with the parsed body. The response
            // phase will replace the upstream's response with the
            // policy-built A2A Task.
            logger::error!(
                "a2a-trace: handle_rpc deferring message/send to response phase"
            );
            Flow::Continue(A2APending::MessageSend {
                id: parsed.id,
                body: body.to_vec(),
                now_unix: now_unix(),
                now_iso: now_iso(),
            })
        }
        "tasks/get" | "tasks/cancel" => {
            // These can be served synchronously when no outbound is
            // required (missing task with `disableObjectStore=true`).
            // When OS2 is enabled and the task exists, the dispatcher
            // makes outbound calls — that path will trap in connected
            // mode and is a known limitation of this v1.
            let id = parsed.id.clone();
            let result = state
                .dispatcher
                .dispatch(&client, parsed, now_unix(), now_iso())
                .await;
            logger::error!("a2a-trace: handle_rpc dispatch returned (sync)");
            let bytes = match result {
                Ok(success) => success.into_bytes(),
                Err(err) => err.into_bytes(),
            };
            // Suppress unused-warning when id is borrowed only for logs.
            let _ = id;
            Flow::Break(make_json_response(200, &bytes))
        }
        _ => {
            // Unknown method - synthesize -32601 immediately, no outbound.
            let err = JsonRpcError::new(
                parsed.id.clone(),
                METHOD_NOT_FOUND,
                format!("Method not found: {}", parsed.method),
            );
            Flow::Break(make_json_response(200, err.into_bytes().as_slice()))
        }
    }
}

/// Response-phase handler. When the request phase deferred a
/// `message/send`, this is where the Salesforce / Agentforce outbound
/// calls happen and the upstream's response is replaced with the
/// policy-built A2A `Task` JSON.
async fn response_filter(
    response: ResponseHeadersState,
    state: PolicyState,
    client: HttpClient,
    data: RequestData<A2APending>,
) {
    let pending = match data {
        RequestData::Continue(pending) => pending,
        RequestData::Break | RequestData::Cancel => {
            // Nothing to do; the request phase already produced the final
            // response or the flow was cancelled.
            return;
        }
    };

    match pending {
        A2APending::PassThrough => {
            // Non-A2A pass-through: leave the upstream response alone.
        }
        A2APending::MessageSend {
            id,
            body,
            now_unix,
            now_iso,
        } => {
            logger::error!("a2a-trace: response_filter handling deferred message/send");

            let parsed = match jsonrpc::parse_request(&body) {
                Ok(r) => r,
                Err(err) => {
                    response.send_response(make_json_response(
                        200,
                        err.into_bytes().as_slice(),
                    ));
                    return;
                }
            };

            // Outbound HTTPS to Salesforce / Agentforce happens here, in
            // the response phase, where connected-mode Flex Gateway does
            // not exhibit the body-state outbound trap.
            let result = state
                .dispatcher
                .dispatch(&client, parsed, now_unix, now_iso)
                .await;
            logger::error!(
                "a2a-trace: response_filter dispatch returned ok={}",
                result.is_ok()
            );

            // Unused now: id is already inside the JsonRpcSuccess/Error.
            let _ = id;

            let bytes = match result {
                Ok(success) => success.into_bytes(),
                Err(err) => err.into_bytes(),
            };

            logger::error!(
                "a2a-trace: response_filter replacing upstream response, len={}",
                bytes.len()
            );
            response.send_response(make_json_response(200, &bytes));
        }
    }
}

/// Fire-once Exchange asset publish. Runs at most once per replica, on
/// the first inbound request after `configure()`. Best-effort: any
/// failure (transport, auth, validation) logs at warn and is swallowed.
async fn maybe_publish_agent_card(state: &PolicyState, client: &HttpClient) {
    if state.publish_done.get() {
        return;
    }
    let publish_cfg = match state.cfg.exchange_publish.as_ref() {
        Some(c) => c,
        None => {
            // Toggle off (or required fields blank): never try, and don't
            // keep checking on every request.
            state.publish_done.set(true);
            return;
        }
    };
    let token_svc = match state.anypoint_token_service.as_ref() {
        Some(s) => s,
        None => {
            logger::warn!(
                "exchange-publish: publishAgentCardToExchange=true but anypointTokenUrl Service is not registered; skipping"
            );
            state.publish_done.set(true);
            return;
        }
    };
    let exch = match state.exchange_service.as_ref() {
        Some(s) => s,
        None => {
            logger::warn!(
                "exchange-publish: publishAgentCardToExchange=true but exchangeBaseUrl Service is not registered; skipping"
            );
            state.publish_done.set(true);
            return;
        }
    };
    let card_bytes = match state.card.warmed_bytes() {
        Some(b) => b,
        None => {
            // URL-source card; not yet resolved. Try again on a later
            // request once the card has been fetched.
            return;
        }
    };

    state.publish_done.set(true);
    let ctx = exchange::publish::PublishContext {
        cfg: state.cfg.as_ref(),
        publish_cfg,
        cache: state.cache.as_ref(),
        anypoint_token_service: token_svc.as_ref(),
        exchange_service: exch.as_ref(),
        anypoint_token_authority: &state.anypoint_token_authority,
        now_unix: now_unix(),
    };
    if let Err(e) =
        exchange::publish::publish_agent_card(client, ctx, card_bytes.as_slice()).await
    {
        logger::warn!(
            "exchange-publish: failed to publish agent-card: {e}; policy continues"
        );
    }
}

fn make_json_response(status: u32, body: &[u8]) -> Response {
    Response::new(status)
        .with_headers(vec![(
            "content-type".to_string(),
            "application/json".to_string(),
        )])
        .with_body(body.to_vec())
}

#[entrypoint]
pub async fn configure(
    launcher: Launcher,
    Configuration(bytes): Configuration,
    cache_builder: CacheBuilder,
) -> anyhow::Result<()> {
    let raw: Config = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow!("invalid policy configuration: {e}"))?;

    let cfg = PolicyConfig::from_raw((&raw).into())
        .map_err(|e| anyhow!("policy configuration rejected: {e}"))?;
    let cfg = Rc::new(cfg);

    let cache: Rc<dyn Cache> = Rc::from(cache_builder.new(CACHE_ID.to_string()).shared().build());

    let Config {
        my_domain_url,
        agentforce_api_url,
        agentforce_api_base_path: cfg_base_path,
        agent_card_url,
        anypoint_token_url,
        exchange_base_url,
        ..
    } = raw;
    let my_domain_service = Rc::new(my_domain_url);
    let agentforce_api_service = Rc::new(agentforce_api_url);
    let agent_card_url_service = agent_card_url.map(Rc::new);
    let anypoint_token_service = anypoint_token_url.map(Rc::new);
    let exchange_service = exchange_base_url.map(Rc::new);
    // Cache-key salt for the Exchange OAuth token. Falls back to a fixed
    // string if no token Service is registered (publish will be skipped
    // anyway, so this is just to keep types straight).
    let anypoint_token_authority = anypoint_token_service
        .as_ref()
        .map(|s| s.uri().authority().to_string())
        .unwrap_or_default();

    let my_domain_authority = my_domain_service.uri().authority().to_string();
    let my_domain_scheme = my_domain_service.uri().scheme().to_string();
    let my_domain_url_value = format!("{my_domain_scheme}://{my_domain_authority}");

    // Prefer the explicit `agentforceApiBasePath` config field (the
    // host-only registered Service is friendlier to connected-mode
    // upstream clusters); fall back to whatever path the registered
    // Service URI carries for backward compatibility.
    let agentforce_api_base_path = match cfg_base_path {
        Some(s) if !s.trim().is_empty() => s,
        _ => agentforce_api_service.uri().path().to_string(),
    };
    logger::info!(
        "agentforce-client: api base path = '{}'",
        agentforce_api_base_path
    );

    let auth = Rc::new(AgentforceAuth::new(
        AgentforceAuthConfig {
            consumer_key: cfg.consumer_key.clone(),
            consumer_secret: cfg.consumer_secret.clone(),
            access_token_override: cfg.agentforce_access_token_override.clone(),
            my_domain_url_for_cache_key: my_domain_authority.clone(),
            cache_safety_margin_seconds: cfg.cache_safety_margin_seconds,
            request_timeout_secs: cfg.agentforce_request_timeout_seconds,
        },
        cache.clone(),
        my_domain_service.clone(),
    ));

    let agentforce_client = Rc::new(AgentforceClient::new(
        auth.clone(),
        agentforce_api_service.clone(),
        agentforce_api_base_path,
        my_domain_url_value,
        cfg.agent_id.clone(),
        cfg.bypass_user,
        cfg.agentforce_request_timeout_seconds,
    ));

    let task_store = Rc::new(TaskStore::new(
        cache.clone(),
        cfg.task_hot_cache_ttl_seconds,
    ));

    let dispatcher = Rc::new(Dispatcher {
        client: agentforce_client.clone(),
        store: task_store.clone(),
    });

    let card = Rc::new(CardProvider::new(
        cfg.clone(),
        cache.clone(),
        agent_card_url_service.clone(),
    ));
    if let Err(e) = card.warm() {
        return Err(anyhow!("agent card configuration rejected: {e}"));
    }

    // Best-effort validation: if the operator turned on the publish
    // toggle but didn't fill in the required fields, log a clear reason
    // here. The publish itself is deferred to the first request.
    if raw.publish_agent_card_to_exchange.unwrap_or(false) && cfg.exchange_publish.is_none() {
        logger::warn!(
            "exchange-publish: publishAgentCardToExchange=true but anypointClientId / anypointClientSecret / anypointOrgId / exchangeAssetId is missing; skipping publish (policy continues)"
        );
    }

    let state = PolicyState {
        cfg: cfg.clone(),
        card,
        dispatcher,
        auth: auth.clone(),
        cache: cache.clone(),
        anypoint_token_service,
        exchange_service,
        anypoint_token_authority,
        publish_done: Rc::new(std::cell::Cell::new(false)),
    };

    let request_state = state.clone();
    let response_state = state;
    let filter = on_request(move |request, client: HttpClient| {
        let state = request_state.clone();
        async move { request_filter(request, state, client).await }
    })
    .on_response(
        move |response, client: HttpClient, data: RequestData<A2APending>| {
            let state = response_state.clone();
            async move { response_filter(response, state, client, data).await }
        },
    );

    launcher.launch(filter).await?;
    Ok(())
}
