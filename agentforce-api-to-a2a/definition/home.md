# Agentforce API to A2A

Wraps the Salesforce **Agentforce Agents API** with an
**[A2A 0.3.0](https://a2a-protocol.org/v0.3.0/specification/)** JSON-RPC
server surface. Apply the policy to any API in API Manager and clients can
discover the agent at `/.well-known/agent-card.json` and exchange messages
over A2A while the policy fans out to Agentforce on the back end.

## What it does

For an API governed at `<flex-host>/<apim-resource>`:

| Inbound                                                  | What the policy does                                              |
|----------------------------------------------------------|-------------------------------------------------------------------|
| `GET <base>/.well-known/agent-card.json`                 | returns the configured `AgentCard` (200 / `application/json`)     |
| `POST <base>` with JSON-RPC `message/send`               | `startSession` (if no `taskId`) -> `sendMessage` (sync) -> `Task` |
| `POST <base>` with JSON-RPC `tasks/get`                  | reads the cached `Task`                                           |
| `POST <base>` with JSON-RPC `tasks/cancel`               | calls Agentforce `endSession` and marks the `Task` `canceled`     |
| anything else                                            | passthrough (or 404 when `strictMode = true`)                     |

A2A `Task.id == Task.contextId == Agentforce sessionId` so multi-turn
conversations round-trip cleanly without a side mapping table.

## Authentication to Salesforce

The policy implements the **OAuth 2.0 Client Credentials Flow** for
Salesforce External Connected Apps documented in
[Get Started with the Agentforce API](https://developer.salesforce.com/docs/ai/agentforce/guide/agent-api-get-started.html):

```bash
curl https://{MY_DOMAIN_URL}/services/oauth2/token \
  --header 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode 'grant_type=client_credentials' \
  --data-urlencode 'client_id={CONSUMER_KEY}' \
  --data-urlencode 'client_secret={CONSUMER_SECRET}'
```

Tokens are cached in the Flex Gateway worker-shared cache, refreshed
proactively `cacheSafetyMarginSeconds` before expiry. If Agentforce ever
returns HTTP 401 the policy evicts the cached token, mints a fresh one,
and retries the same upstream call exactly once before surfacing a
JSON-RPC `-32603 InternalError` with `data.reason = agentforce_auth_rejected`.

## Task persistence (Anypoint Object Store v2)

A2A `Task` state is persisted to **Anypoint Object Store v2** so the same
session is reachable from any Flex Gateway replica. A short-lived
`taskHotCacheTtlSeconds` PDK shared cache fronts OS v2 to absorb
follow-on reads. Writes are best-effort - a transient OS v2 outage logs
a warning and lets the RPC succeed; subsequent `tasks/get` against that
task will simply return `TaskNotFoundError -32001`.

When `autoCreateStore` is `true` (the default), the policy POSTs to the
Anypoint OS v2 admin endpoint on first use to materialize the store if
it does not yet exist. The connected app supplied via `anypointClientId`
must have the **Use Object Store with the connected app** scope, plus
**Object Store Admin** when `autoCreateStore` is on. Set
`autoCreateStore: false` if you would rather pre-create the store
yourself.

Set `disableObjectStore: true` as an escape hatch when OS v2 is
unhealthy; the policy will keep serving the A2A surface using only the
per-replica in-memory hot cache. Tasks become non-durable in that mode
(`tasks/get` for tasks written on a different replica returns
`TaskNotFoundError -32001`), but `message/send` keeps working.

## How outbound calls are dispatched (`on_request` + `on_response`)

For requests that route to `message/send`, the policy reads the inbound
body in `on_request` and returns `Flow::Continue`, then performs all
Salesforce / Agentforce / OS v2 calls in `on_response` and replaces the
upstream's response with the policy-built A2A `Task` JSON. The other
methods (`tasks/get`, `tasks/cancel`, malformed JSON, unknown method,
agent-card discovery) short-circuit synchronously in `on_request`.

This split exists because, on some connected-mode Flex Gateway runtime
versions, outbound HTTPS calls issued from a PDK WASM filter *after*
the request body has been read cause the filter to trap. Issuing them
from the response phase routes around the issue. No operator action is
required; the architecture is internal.

## Agent card configuration

Pick one of four sources via `agentCardSource`. Whichever you choose,
the policy **always overrides** `protocolVersion`, `url`, and
`preferredTransport` on the served card so it cannot lie about where it
is hosted.

### `structured` (default)

Fill in the form fields:

```yaml
agentCardSource: structured
agentCardName: Acme Agentforce Agent
agentCardDescription: A2A wrapper around the Agentforce Agents API.
agentCardVersion: "1.0.0"
agentCardProviderOrganization: Acme, Inc.
agentCardCapabilitiesStreaming: false
agentCardCapabilitiesPushNotifications: false
agentCardDefaultInputModes: text/plain
agentCardDefaultOutputModes: text/plain
agentCardSkillsJson: |
  [
    {
      "id": "general-conversation",
      "name": "General Conversation",
      "description": "Sync conversation with the agent.",
      "tags": ["agentforce"]
    }
  ]
```

### `inline_json`

Paste a complete card into `agentCardJson`. Useful when you want full
control over every field including the security schemes:

```yaml
agentCardSource: inline_json
agentCardJson: |
  {
    "name": "Acme Agentforce Agent",
    "description": "Wraps Agentforce.",
    "version": "1.0.0",
    "capabilities": { "streaming": false, "pushNotifications": false },
    "defaultInputModes": ["text/plain"],
    "defaultOutputModes": ["text/plain"],
    "skills": [{
      "id": "general-conversation",
      "name": "General Conversation",
      "description": "Sync conversation",
      "tags": ["agentforce"]
    }]
  }
```

### `file`

Same field as `inline_json`. The Anypoint API Manager UI does not have a
file-upload widget so paste the contents of your `agent-card.json`:

```bash
$ cat agent-card.json | pbcopy   # macOS
# Then paste into the agentCardJson textarea in API Manager.
```

The discriminator value `file` exists so operators can declare intent
("this card came from a file we ship with our app").

### `url`

The policy registers the URL as an Envoy upstream cluster and fetches the
card lazily on the first `/.well-known/agent-card.json` request, then
memoizes the result for ~10 minutes. Redeploy the policy (or wait for
the cache to expire) to refresh:

```yaml
agentCardSource: url
agentCardUrl: https://internal-cdn.acme.com/cards/agentforce.json
```

### `agentCardOverrideJson` (any source)

Optional JSON object deep-merged on top of whichever card resolved
above. Use it to override a single field without rewriting the whole
card, e.g. add a security scheme:

```yaml
agentCardOverrideJson: |
  {
    "securitySchemes": {
      "MuleSoftClientId": {
        "type": "apiKey",
        "in": "header",
        "name": "client_id"
      }
    }
  }
```

## Quick start: hit the API

After deploying, your operator-supplied URL exposes the A2A surface.
`<base>` here is the proxy path you configured for the API instance
(e.g. `https://your-flex.example.com/agentforce-a2a`). `<rpc>` is
`<base>` plus `a2aRpcPath` (default `/a2a/v1/rpc`); A2A clients pick
this up automatically from `AgentCard.url`.

```bash
# 1. Discover the agent. A2A clients read `AgentCard.url` from this
#    response and POST follow-up RPC requests there.
curl https://your-flex.example.com/agentforce-a2a/.well-known/agent-card.json

# 2. Send a message (no taskId yet -> a fresh Agentforce session is started).
curl https://your-flex.example.com/agentforce-a2a/a2a/v1/rpc \
  -H 'content-type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "message/send",
    "params": {
      "message": {
        "kind": "message",
        "messageId": "u-001",
        "role": "user",
        "parts": [{"kind":"text","text":"hello"}]
      }
    }
  }'

# 3. Continue the conversation. Take `result.id` from the previous
#    response and pass it back as `taskId` (and optionally `contextId`).
#    The policy reuses the same Agentforce session and skips startSession,
#    so the agent's reply is context-aware.
curl https://your-flex.example.com/agentforce-a2a/a2a/v1/rpc \
  -H 'content-type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "message/send",
    "params": {
      "message": {
        "kind": "message",
        "messageId": "u-002",
        "role": "user",
        "taskId": "<previous result.id>",
        "contextId": "<previous result.id>",
        "parts": [{"kind":"text","text":"check inventory for MULETEST0"}]
      }
    }
  }'

# 4. Retrieve the persisted Task by id (== Agentforce sessionId).
#    Requires `disableObjectStore: false` so the policy can read it
#    back from OS v2.
curl https://your-flex.example.com/agentforce-a2a/a2a/v1/rpc \
  -H 'content-type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "id": 3,
    "method": "tasks/get",
    "params": { "id": "<previous result.id>" }
  }'

# 5. Cancel the conversation (calls Agentforce endSession).
curl https://your-flex.example.com/agentforce-a2a/a2a/v1/rpc \
  -H 'content-type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "id": 4,
    "method": "tasks/cancel",
    "params": { "id": "<previous result.id>" }
  }'
```

## Configuration reference

| Field | Required | Default | Description |
|---|---|---|---|
| `myDomainUrl` | yes | — | Salesforce My Domain URL. The OAuth `/services/oauth2/token` endpoint is reached at `<myDomainUrl>/services/oauth2/token`. |
| `agentforceApiUrl` | yes | `https://api.salesforce.com` | Host-only base URL of the Agentforce Agents API. Registered as an Envoy upstream cluster, so it must be host-only (no path). |
| `agentforceApiBasePath` | no | `/einstein/ai-agent/v1` | Path prefix prepended to every Agentforce REST call (combined with `agentforceApiUrl`). |
| `consumerKey` | yes | — | External Connected App OAuth `client_id`. |
| `consumerSecret` | yes | — | External Connected App OAuth `client_secret`. Reference via Anypoint Secrets Manager. |
| `agentforceAccessTokenOverride` | no | — | Diagnostic override. When set, the policy skips the Salesforce OAuth exchange and uses this bearer for Agentforce calls. Leave blank in production. |
| `agentId` | yes | — | Agentforce agent id used in `POST /agents/{id}/sessions`. |
| `bypassUser` | no | `true` | Use the agent-assigned user (correct for `client_credentials`). |
| `cacheSafetyMarginSeconds` | no | `60` | Refresh tokens this many seconds before `expires_in`. |
| `agentforceRequestTimeoutSeconds` | no | `180` | Per-call HTTP timeout (s) for every outbound Agentforce call. Overrides the PDK client's 10s default, which is too low for long RAG turns and shows up as a spurious 504. Capped at `290` (10s under the gateway's 300s response timeout) so the policy times out first with a clean JSON-RPC error. |
| `protocolVersion` | no | `0.3.0` | A2A protocol version. Only `0.3.0` is supported in v1. |
| `a2aRpcPath` | no | `/` | Relative path under the governed API where JSON-RPC is accepted. |
| `publicBaseUrl` | yes | — | Externally reachable base URL, written into `AgentCard.url`. |
| `strictMode` | no | `false` | Return 404 for non-A2A requests instead of forwarding upstream. |
| `objectStoreBaseUrl` | yes | — | Region-scoped Anypoint Object Store v2 host. |
| `anypointTokenUrl` | yes | `https://anypoint.mulesoft.com/accounts/api/v2/oauth2/token` | Anypoint platform OAuth token endpoint. |
| `anypointClientId` | yes | — | Anypoint connected-app client id. |
| `anypointClientSecret` | yes | — | Anypoint connected-app client secret. |
| `anypointOrgId` | yes | — | Anypoint organization UUID. |
| `anypointEnvId` | yes | — | Anypoint environment UUID. |
| `objectStoreId` | yes | — | OS v2 store name. |
| `autoCreateStore` | no | `true` | When true, the policy creates the OS v2 store on first use if it doesn't exist. Requires *Object Store Admin* scope on the connected app. |
| `disableObjectStore` | no | `false` | Escape hatch. When true, the policy keeps tasks only in the per-replica in-memory hot cache (no OS v2 calls). |
| `objectStoreTtlSeconds` | no | `86400` | TTL applied when the policy creates the OS v2 store. Ignored if the store already exists. |
| `taskHotCacheTtlSeconds` | no | `60` | TTL of the per-replica PDK hot cache in front of OS v2. |
| `taskStoreTimeoutMs` | no | `1500` | Per-call timeout for OS v2 reads/writes. |
| `agentCardSource` | no | `structured` | `inline_json` \| `url` \| `structured` \| `file`. |
| `agentCardJson` | conditional | — | Inline JSON for `inline_json` or `file` modes. |
| `agentCardUrl` | conditional | — | URL fetched at runtime for `url` mode. |
| `agentCard*` (structured fields) | no | — | Form fields used when `agentCardSource = structured`. |
| `agentCardOverrideJson` | no | — | JSON object deep-merged on top of the resolved card. |

## Out of scope for v1

- `message/stream` (Server-Sent Events).
- A2A push notifications.
- A2A `FilePart` / `DataPart` (only `TextPart` is wired in v1).
- gRPC and HTTP+JSON A2A transports.
- `agentCardSignatures` (signing the card with JWS).
