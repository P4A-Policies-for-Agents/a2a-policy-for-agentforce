# `agentforce-api-to-a2a` Policy

Flex Gateway custom policy that fronts the Salesforce **Agentforce Agents API** with the
**A2A Protocol 0.3.0** JSON-RPC server surface. See [`definition/home.md`](definition/home.md)
for the operator-facing documentation, and [`ROADMAP.md`](ROADMAP.md) for known gaps and
follow-ups.

This policy was created with the Flex Gateway Policy Development Kit (PDK) 1.8.0. To find
the complete PDK documentation, see
[PDK Overview](https://docs.mulesoft.com/pdk/latest/policies-pdk-overview).


## What it does

Translates A2A JSON-RPC requests into Agentforce Agents API calls:

| A2A method     | Agentforce call                                              |
|----------------|--------------------------------------------------------------|
| `message/send` | `POST /agents/{id}/sessions` then `POST /sessions/{id}/messages` |
| `tasks/get`    | hot-cache + Object Store v2 lookup                           |
| `tasks/cancel` | `DELETE /sessions/{id}` and marks the task `canceled`        |

It also serves the configured `AgentCard` at `<base>/.well-known/agent-card.json`.

The runtime is split into a request-phase and a response-phase filter so outbound HTTPS
calls to Salesforce/Agentforce happen in the response phase. This routes around a
connected-mode Flex Gateway WASM body-state outbound trap; see [`ROADMAP.md`](ROADMAP.md)
for the underlying issue.


## Deploying to your own org

Step-by-step. Skip steps you already have set up.

> **Before you build or deploy, update the configuration with your own values.**
> The config ships with placeholder Salesforce and Anypoint values (My Domain URL,
> consumer key/secret, agent id, Anypoint credentials, Object Store settings) that
> will not work against your org until you replace them:
> - **Local smoke test (step 6):** edit `playground/config/api.yaml`.
> - **Anypoint deployment (step 8):** edit the `policy-config.json` you apply in
>   API Manager.
>
> Also update the agent card fields — `agentCardName`, `agentCardDescription`, and the
> `agentCardSkillsJson` skills — so the published AgentCard describes *your* Agentforce
> agent rather than the example "Sales and Distribution Agent". These are what A2A
> clients see at `/.well-known/agent-card.json`.
>
> Do this before running `make build` / `make run` and before applying the policy.

### Prerequisites

- A Salesforce org with **Agentforce** enabled, including at least one agent and an
  **External Connected App** configured for the **Client Credentials Flow**.
- An **Anypoint Platform** account with API Manager and Runtime Manager access.
- A **Flex Gateway** running and registered in either:
  - **Local mode** — Docker Desktop (≥ 4.x), or
  - **Connected mode** — CloudHub managed gateway, EKS / RTF self-managed gateway, etc.
- macOS or Linux developer machine with:
  - **Rust** toolchain (`rustup` recommended).
  - **Docker Desktop** running.
  - **Anypoint CLI v4** (`anypoint-cli-v4`), authenticated to your org.
- Optional but recommended: an **Anypoint connected app** with the
  *Use Object Store with the connected app* scope, for persistent task storage.

### 1. Clone and bootstrap

```bash
git clone <this-repo>
cd agentforce-api-to-a2a
make setup
```

`make setup` installs `cargo-anypoint` and downloads PDK dependencies.

### 2. Set the asset coordinates to your Anypoint org

Open `Cargo.toml` and replace the `group_id` under `[package.metadata.anypoint]` with your
**Anypoint organization UUID** (find it in Anypoint Platform > Access Management > Business
Groups, or in any URL like `https://anypoint.mulesoft.com/exchange/<group_id>/...`):

```toml
[package.metadata.anypoint]
group_id = "<YOUR-ANYPOINT-ORG-UUID>"
definition_asset_id = "agentforce-api-to-a-2-a"
implementation_asset_id = "agentforce-api-to-a-2-a-wasm"
```

You can change the asset ids if you'd like a different name; Anypoint Exchange asset id
rules require each hyphen-separated segment to be purely alphabetic or purely numeric, so
`a2a` becomes `a-2-a`.

### 3. Set up Salesforce

In Salesforce:

1. **Create or pick an Agentforce agent** and copy its agent id (`0Xx…`) — it appears in
   the agent's URL in Setup. You'll use this as `agentId`.
2. **Create an External Connected App** with the *Client Credentials Flow* enabled
   ([Salesforce docs](https://developer.salesforce.com/docs/ai/agentforce/guide/agent-api-get-started.html)).
   Note the **Consumer Key** (`consumerKey`) and **Consumer Secret** (`consumerSecret`).
3. **Verify locally** with curl that the credentials work:

   ```bash
   curl https://<your-my-domain>.my.salesforce.com/services/oauth2/token \
     -H 'Content-Type: application/x-www-form-urlencoded' \
     --data-urlencode 'grant_type=client_credentials' \
     --data-urlencode 'client_id=<consumerKey>' \
     --data-urlencode 'client_secret=<consumerSecret>'
   ```

   You should get a JSON response with `access_token` and `instance_url`.

### 4. (Optional) Set up Anypoint Object Store v2

Only needed if you want **persistent task storage and full multi-turn conversation
history** in the response. If skipped, set `disableObjectStore: true` in the policy
config.

1. In Anypoint, **create a connected app** with the
   *Use Object Store with the connected app* scope (and *Object Store Admin* if you want
   the policy to auto-create the store on first use).
2. Note the **client id** (`anypointClientId`) and **client secret**
   (`anypointClientSecret`).
3. Note your **organization UUID** (`anypointOrgId`) and **environment UUID**
   (`anypointEnvId`). The org UUID is the same as `group_id` in `Cargo.toml`. The env UUID
   is in Anypoint Platform → Access Management → Environments.
4. Decide on a store name (`objectStoreId`), e.g. `agentforce-a2a-tasks`.

> **Note**: at the time of writing, `tasks/get` and `tasks/cancel` still dispatch
> synchronously in the request phase, which trips the connected-mode WASM body-state
> outbound trap when `disableObjectStore: false`. Track [`ROADMAP.md`](ROADMAP.md) #1 for
> the fix. Until that lands, leave `disableObjectStore: true`.

### 5. Build and unit-test

```bash
make build
cargo test --release
```

You should see all unit and integration tests pass.

### 6. Smoke-test locally with Docker

This step exercises the full policy end-to-end against a real Salesforce + real
Agentforce, on a local Flex Gateway in `--mode=local`.

1. Generate a local-mode `registration.yaml` (you'll need a one-time registration token
   from Anypoint Platform → Runtime Manager → Flex Gateway → Add Gateway → Local mode):

   ```bash
   docker run --rm --platform linux/amd64 \
     --entrypoint /usr/local/bin/flexctl \
     -v "$(pwd)/playground/config:/registration" \
     mulesoft/flex-gateway:latest \
     registration create \
       --mode=local \
       --token=<one-time-token> \
       --organization=<your-anypoint-org-uuid> \
       --output-directory=/registration \
       <your-gateway-name>
   ```

   This drops `playground/config/registration.yaml`. Add it to your `.gitignore` — it
   contains a long-lived credential.

2. Edit `playground/config/api.yaml` and replace the placeholder values under
   `spec.policies[0].config` with your real values from steps 3 and 4. At a minimum:
   `myDomainUrl`, `consumerKey`, `consumerSecret`, `agentId`, `publicBaseUrl`, plus the
   Anypoint OS2 fields (or `disableObjectStore: true`).

3. Stage the freshly built artifacts into the playground:

   ```bash
   cp target/wasm32-wasip1/release/agentforce_api_to_a2a_implementation.yaml \
      playground/config/custom-policies/
   cp target/wasm32-wasip1/release/agentforce_api_to_a2a_definition.yaml \
      playground/config/custom-policies/
   ```

   On `mulesoft/flex-gateway:latest`, the local mode parser rejects `format: password` in
   the definition file. Strip those lines (only locally — leave the source `gcl.yaml`
   alone):

   ```bash
   python3 -c '
   from pathlib import Path
   p = Path("playground/config/custom-policies/agentforce_api_to_a2a_definition.yaml")
   p.write_text(p.read_text().replace("      format: password\n", ""))'
   ```

4. Bring up the gateway:

   ```bash
   docker compose -f playground/docker-compose.yaml up -d
   docker compose -f playground/docker-compose.yaml logs -f local-flex
   ```

5. Test the endpoints:

   ```bash
   curl http://localhost:8081/.well-known/agent-card.json | jq .

   curl -sS -X POST http://localhost:8081/a2a/v1/rpc \
     -H 'content-type: application/json' \
     --data '{
       "jsonrpc":"2.0","id":1,
       "method":"message/send",
       "params":{
         "message":{
           "kind":"message","messageId":"u-001","role":"user",
           "parts":[{"kind":"text","text":"hello"}]
         }
       }
     }' | jq .
   ```

   The first call returns a `Task` with the agent's reply. To continue the conversation,
   pass the returned `result.id` as `taskId` on subsequent `message/send` calls.

   When done:

   ```bash
   docker compose -f playground/docker-compose.yaml down
   ```

### 7. Publish to Anypoint Exchange

Once the local smoke test works, publish a development build:

```bash
make publish
```

This produces:

- `<group_id>:agentforce-api-to-a-2-a-dev:<version>` — the policy definition asset.
- `<group_id>:agentforce-api-to-a-2-a-wasm-dev:<version>` — the policy implementation
  asset (the WASM binary).

The version is derived from `Cargo.toml` plus a build timestamp. To publish a release
asset (no `-dev` suffix, immutable version), use `make release` after bumping the version
in `Cargo.toml`.

### 8. Apply the policy to an API in API Manager

You need an **API instance** (Flex Gateway technology, deployed to your gateway target).
You can create one in the Anypoint UI or via CLI.

#### 8a. Create the API instance (CLI)

```bash
anypoint-cli-v4 api-mgr api manage \
  <some-asset-id-in-exchange> 1.0.0 \
  -f -p \
  --apiInstanceLabel agentforce-a2a \
  --uri https://api.salesforce.com \
  --port 80 \
  --path /agentforce-a2a \
  --scheme http \
  --type http \
  --deploymentType hybrid \
  --environment <your-env-name>
```

Note the **API instance id** that the command prints; you'll use it below.

> The upstream URI (`--uri`) doesn't really matter for this policy — the policy
> short-circuits for the A2A surface and replaces the response in the response phase. Any
> reachable HTTP endpoint that returns 2xx works (`https://api.salesforce.com` is fine and
> avoids needing an extra service).

#### 8b. Deploy the API instance to your Flex Gateway

```bash
anypoint-cli-v4 api-mgr api deploy <api-instance-id> \
  --target <flex-gateway-id> \
  --gatewayVersion 1.13.0 \
  --environment <your-env-name>
```

The gateway id is the UUID printed by
`anypoint-cli-v4 runtime-mgr gateways managed list` (or `gateways self-managed list` for
self-managed gateways like EKS/RTF).

#### 8c. Apply the policy

Save your real values into `policy-config.json`:

```json
{
  "myDomainUrl": "https://<your-my-domain>.my.salesforce.com",
  "agentforceApiUrl": "https://api.salesforce.com",
  "agentforceApiBasePath": "/einstein/ai-agent/v1",
  "consumerKey": "<consumerKey>",
  "consumerSecret": "<consumerSecret>",
  "agentId": "<0Xx-agent-id>",
  "bypassUser": true,
  "cacheSafetyMarginSeconds": 60,
  "agentforceRequestTimeoutSeconds": 180,

  "protocolVersion": "0.3.0",
  "a2aRpcPath": "/a2a/v1/rpc",
  "publicBaseUrl": "https://<gateway-public-host>/<api-instance-path>",
  "strictMode": true,

  "objectStoreBaseUrl": "https://object-store-us-east-1.anypoint.mulesoft.com",
  "anypointTokenUrl": "https://anypoint.mulesoft.com/accounts/api/v2/oauth2/token",
  "anypointClientId": "<anypoint-cc-client-id>",
  "anypointClientSecret": "<anypoint-cc-client-secret>",
  "anypointOrgId": "<your-anypoint-org-uuid>",
  "anypointEnvId": "<your-anypoint-env-uuid>",
  "objectStoreId": "agentforce-a2a-tasks",
  "autoCreateStore": true,
  "disableObjectStore": true,
  "objectStoreTtlSeconds": 86400,

  "agentCardSource": "structured",
  "agentCardName": "My Agentforce Agent",
  "agentCardDescription": "Agentforce agent fronted by A2A.",
  "agentCardVersion": "1.0.0",
  "agentCardProviderOrganization": "My Org",
  "agentCardProviderUrl": "https://example.com",
  "agentCardCapabilitiesStreaming": false,
  "agentCardCapabilitiesPushNotifications": false,
  "agentCardDefaultInputModes": "text/plain",
  "agentCardDefaultOutputModes": "text/plain",
  "agentCardSkillsJson": "[{\"id\":\"chat\",\"name\":\"Chat\",\"description\":\"General conversation\",\"tags\":[\"agentforce\"]}]"
}
```

Then apply:

```bash
anypoint-cli-v4 api-mgr policy apply <api-instance-id> agentforce-api-to-a-2-a-dev \
  --policyVersion <version-from-make-publish> \
  --groupId <your-anypoint-org-uuid> \
  --configFile ./policy-config.json \
  --environment <your-env-name>
```

`<version-from-make-publish>` looks like `1.0.0-20260508202029`. Use the same value for
`agentforce-api-to-a-2-a-dev` published in step 7.

> **Tip**: keep `disableObjectStore: true` for the first deployment. After verification,
> see [`ROADMAP.md`](ROADMAP.md) before flipping it to `false`.

### 9. Verify the deploy

Run the test ladder below to confirm every part of the policy is wired up correctly.
Substitute `URL` with your deployed proxy base (the value you used for `publicBaseUrl`).
`RPC` is `URL` plus the configured `a2aRpcPath` (default `/a2a/v1/rpc`).

```bash
# Set to your deployed proxy base. No trailing slash.
URL='https://<flex-host>/<api-instance-path>'
RPC="$URL/a2a/v1/rpc"
```

#### T1 — Agent card discovery

```bash
curl -sS -i "$URL/.well-known/agent-card.json" | head -5
echo
curl -sS "$URL/.well-known/agent-card.json" \
  | jq '{name, protocolVersion, preferredTransport, url, skills: (.skills|length)}'
```

**Expected**: `200 OK` with `content-type: application/json` and a body containing your
configured agent name, `protocolVersion: "0.3.0"`, `preferredTransport: "JSONRPC"`, the
public `url` you configured, and the count of skills you registered.

#### T2 — Malformed JSON

```bash
curl -sS -X POST "$RPC" \
  -H 'content-type: application/json' \
  --data 'not-json-at-all'
```

**Expected**:

```json
{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error","data":{"reason":"<details>"}}}
```

A2A spec error `-32700 Parse error`.

#### T3 — Unknown RPC method

```bash
curl -sS -X POST "$RPC" \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"foobar/baz","params":{}}'
```

**Expected**:

```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found: foobar/baz"}}
```

A2A spec error `-32601 Method not found`.

#### T4 — `tasks/cancel` for a missing task

```bash
curl -sS -X POST "$RPC" \
  -H 'content-type: application/json' \
  --data '{
    "jsonrpc":"2.0","id":2,
    "method":"tasks/cancel",
    "params":{ "id":"does-not-exist" }
  }'
```

**Expected**:

```json
{"jsonrpc":"2.0","id":2,"error":{"code":-32001,"message":"Task not found","data":{"taskId":"does-not-exist"}}}
```

A2A spec error `-32001 TaskNotFoundError`.

#### T5 — `tasks/get`

`tasks/get` has two paths: a missing task returns `-32001`, an existing task returns the
persisted A2A `Task` JSON. The "existing" half depends on a task being readable from
the policy's task store (either the per-replica hot cache within
`taskHotCacheTtlSeconds`, or Anypoint Object Store v2 when `disableObjectStore: false`).
Run T6 first to create a task, then come back to T5b.

##### T5a — Missing task (negative)

```bash
curl -sS -X POST "$RPC" \
  -H 'content-type: application/json' \
  --data '{
    "jsonrpc":"2.0","id":3,
    "method":"tasks/get",
    "params":{ "id":"does-not-exist" }
  }'
```

**Expected**: same as T4 — `200` with `-32001 Task not found`.

##### T5b — Existing task (positive, run after T6 / T7)

Reuses `$TID` produced by T6 (and updated by T7).

```bash
curl -sS -X POST "$RPC" \
  -H 'content-type: application/json' \
  --data "{
    \"jsonrpc\":\"2.0\",\"id\":3,
    \"method\":\"tasks/get\",
    \"params\":{ \"id\":\"$TID\" }
  }" | jq .
```

**Expected**: `200` with the full A2A `Task` for `$TID`. Shape:

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "id": "<task-id>",
    "kind": "task",
    "contextId": "<task-id>",
    "status": { "state": "completed", "timestamp": "<ISO-8601>" },
    "history": [
      { "kind":"message", "role":"user",  "messageId":"<u-msg-id>",        "parts":[{"kind":"text","text":"<last user turn>"}],        "taskId":"<task-id>", "contextId":"<task-id>" },
      { "kind":"message", "role":"agent", "messageId":"<agent-msg-id>",    "parts":[{"kind":"text","text":"<last agent reply>"}],      "taskId":"<task-id>", "contextId":"<task-id>" }
    ],
    "artifacts": [
      { "artifactId":"agent-response-<agent-msg-id>", "name":"agent-response", "parts":[ { "kind":"text", "text":"<last agent reply>" } ] }
    ]
  }
}
```

Optional — request only the last N turns with `historyLength`:

```bash
curl -sS -X POST "$RPC" \
  -H 'content-type: application/json' \
  --data "{
    \"jsonrpc\":\"2.0\",\"id\":4,
    \"method\":\"tasks/get\",
    \"params\":{ \"id\":\"$TID\", \"historyLength\":1 }
  }" | jq '.result.history | length'
```

Caveats:

- If `disableObjectStore: true` (the current default), the task is only readable from
  the **same gateway replica** that handled T6 / T7, and only within
  `taskHotCacheTtlSeconds` (default 60s). A request landing on a different replica or
  arriving later will fall back to `-32001`.
- With `disableObjectStore: false`, `tasks/get` reads from Object Store v2 and is
  reliable across replicas. See [`ROADMAP.md`](ROADMAP.md) #1 / #2 for the prerequisites
  to flip that flag.
- `result.history` only includes turns the policy persisted. With OS v2 disabled, the
  history reflects whatever the last `message/send` on that replica stored — typically
  the most recent two turns (user + agent).

#### T6 — `message/send` (turn 1, no `taskId` → fresh Agentforce session)

```bash
TID=$(curl -sS -X POST "$RPC" \
  -H 'content-type: application/json' \
  --data '{
    "jsonrpc":"2.0","id":10,
    "method":"message/send",
    "params":{
      "message":{
        "kind":"message",
        "messageId":"u-001",
        "role":"user",
        "parts":[{"kind":"text","text":"<your-first-user-message>"}]
      }
    }
  }' | tee /tmp/t6.json | jq -r .result.id)

echo "task id = $TID"
jq -r '.result.history[1].parts[0].text' /tmp/t6.json
```

**Expected**: `200` with a JSON-RPC envelope wrapping an A2A `Task`:

```json
{
  "jsonrpc": "2.0",
  "id": 10,
  "result": {
    "id": "<task-id>",
    "kind": "task",
    "contextId": "<task-id>",
    "status": { "state": "completed", "timestamp": "<ISO-8601>" },
    "history": [
      { "kind":"message", "role":"user",  "messageId":"u-001",          "parts":[{"kind":"text","text":"<your-first-user-message>"}], "taskId":"<task-id>", "contextId":"<task-id>" },
      { "kind":"message", "role":"agent", "messageId":"<agent-msg-id>", "parts":[{"kind":"text","text":"<agent reply>"}],              "taskId":"<task-id>", "contextId":"<task-id>" }
    ],
    "artifacts": [ { "artifactId":"agent-response-<agent-msg-id>", "name":"agent-response", "parts":[...] } ]
  }
}
```

The `result.id` is the **task id** == the Agentforce session id. Reuse it in T7.

#### T7 — `message/send` (turn 2, same `taskId` → multi-turn)

```bash
curl -sS -X POST "$RPC" \
  -H 'content-type: application/json' \
  --data "{
    \"jsonrpc\":\"2.0\",\"id\":11,
    \"method\":\"message/send\",
    \"params\":{
      \"message\":{
        \"kind\":\"message\",
        \"messageId\":\"u-002\",
        \"role\":\"user\",
        \"taskId\":\"$TID\",
        \"contextId\":\"$TID\",
        \"parts\":[{\"kind\":\"text\",\"text\":\"<your-follow-up-message>\"}]
      }
    }
  }" | tee /tmp/t7.json | jq '{returned_task: .result.id, same_as_t6: (.result.id == "'"$TID"'")}'

jq -r '.result.history[1].parts[0].text' /tmp/t7.json
```

**Expected**: `200` with `result.id == $TID` (the same task is reused, no new session is
minted) and `result.history[1].parts[0].text` being a context-aware reply that depends
on T6 (the agent remembers what was discussed). The policy reuses the Agentforce
session id under the hood.

#### Summary

| Test | Method + path | Expected status | Expected body shape |
|---|---|---|---|
| T1 — agent card | `GET <url>/.well-known/agent-card.json` | `200` | Full `AgentCard` JSON with configured `name`, `skills`, and `url == <RPC>` |
| T2 — malformed | `POST <RPC>` with non-JSON | `200` | `-32700 Parse error` |
| T3 — unknown method | `POST <RPC>` with `method: "foobar/baz"` | `200` | `-32601 Method not found: foobar/baz` |
| T4 — cancel missing | `POST <RPC>` with `tasks/cancel` for nonexistent task | `200` | `-32001 Task not found` |
| T5a — get missing | `POST <RPC>` with `tasks/get` for nonexistent task | `200` | `-32001 Task not found` |
| T5b — get existing | `POST <RPC>` with `tasks/get` for `$TID` from T6 | `200` | Full A2A `Task` JSON (id, status, history, artifacts) |
| T6 — fresh `message/send` | `POST <RPC>` with `message/send`, no `taskId` | `200` | A2A `Task` with one user + one agent turn |
| T7 — same-task `message/send` | `POST <RPC>` with `message/send`, `taskId = <T6 id>` | `200` | Same `task.id`, context-aware agent reply |

#### Failure-mode reading

If any test returns an unexpected response, the body usually tells you which subsystem is at fault:

| Response | Cause |
|---|---|
| `404` + `{"error":"not_found","reason":"strictMode"}` | You hit the wrong path. The policy is alive but classified your request as passthrough. Hit `<url>/.well-known/agent-card.json` and read `.url` for the correct RPC endpoint. |
| `404` empty body, fast (< 200ms) | Gateway-level route mismatch. The proxy path you're calling doesn't match any deployed API instance. Verify via `anypoint-cli-v4 api-mgr api list`. |
| `404` empty body, no `content-type`, with `x-envoy-decorator-operation` | Connected-mode WASM trap. Confirm the gateway is on the release build and the policy includes the on-response refactor; see [`ROADMAP.md`](ROADMAP.md) #1 / #3. |
| `200` `-32603 InternalError` with `data.reason = agentforce_auth_rejected` | Salesforce credentials wrong / expired / not authorized for this agent. |
| `200` `-32603` with `data.reason = agentforce_http_error` and `status: 4xx` | Agentforce API rejected the request. Check `agentId`, the agent's user assignment, and `bypassUser`. |
| `200` `-32603` with `data.reason = agentforce_transport_error` | Either network reachability from the gateway to `api.salesforce.com` is broken, **or** the agent turn exceeded `agentforceRequestTimeoutSeconds` (default 180s) and the policy's own HTTP timeout fired. For a slow agent, raise the field (max 290). |
| Bare `504` with no `data.reason`, after ~300s | The gateway's upstream response timeout fired before the policy — `agentforceRequestTimeoutSeconds` is set at/above the gateway timeout. Lower it below the gateway ceiling (max 290). A bare `504` after ~60s instead points at the gateway's connection-idle timeout, which no policy setting can override for a synchronous turn. |

#### Notes

- Replace `<flex-host>/<api-instance-path>` with your deploy's actual URL — same value
  as the `publicBaseUrl` you set in the policy config, and what `AgentCard.url` will
  declare (minus the `a2aRpcPath` suffix).
- T7 multi-turn caveat: if the gateway has multiple replicas and
  `disableObjectStore: true`, the second request can land on a different replica whose
  hot cache doesn't have the task. The agent reply still works (Salesforce keeps session
  state), but the policy-returned `Task` may only contain the second turn's history.
  Enable OS v2 (see [`ROADMAP.md`](ROADMAP.md) #1 / #2) for cross-replica persistence.


## Make command reference

This project has a Makefile that includes different goals that assist the developer
during the policy development lifecycle. For more information about the Makefile, see
[Makefile](https://docs.mulesoft.com/pdk/latest/policies-pdk-create-project#makefile).

### Setup

`make setup` installs the Policy Development Kit internal dependencies for the rest of
the Makefile goals.

### Build

`make build` compiles the WebAssembly binary of the policy and regenerates the policy
asset files (`definition.yaml`, `implementation.yaml`, `gcl.yaml`).

### Run

`make run` runs the current build of the policy in a containerized Flex Gateway. The
`playground/config/registration.yaml` file generated by a Flex Gateway local-mode
registration must be present. See deploy step 6 above.

### Test

`make test` runs unit tests and integration tests. Equivalent to
`cargo test --release`.

### Publish / Release

`make publish` publishes a development asset (`-dev` suffix, mutable timestamp version).
`make release` publishes the immutable release asset using the version from `Cargo.toml`.
See `definition/home.md` for the published asset documentation.
