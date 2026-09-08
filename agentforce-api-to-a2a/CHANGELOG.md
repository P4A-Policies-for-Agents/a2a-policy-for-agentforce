# Changelog

All notable changes to the `agentforce-api-to-a2a` policy are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Changes
that are merged but not yet released live under **[Unreleased]**; when a release is cut, that
heading is renamed to the version and dated (e.g. `## [1.1.0] - 2026-08-09`).

## [Unreleased]

### Added

- **Configurable Agentforce request timeout.** New `agentforceRequestTimeoutSeconds` policy
  config field (integer, clamped to `1`–`290`, default `180`). It sets the per-call HTTP
  timeout for every outbound Agentforce REST call — OAuth token, `startSession`,
  `sendMessage`, and `endSession`.

### Fixed

- **OAuth token exchange no longer fails when `expires_in` is a JSON string.** The Salesforce
  token endpoint returns `"expires_in":"7200"` (a string) where it previously returned a
  number. `TokenResponse.expires_in` is typed `u64`, so every token exchange failed
  deserialization and every `message/send` returned `-32603` with
  `data.reason = agentforce_token_failure` — with no error logged, because the `BadJson` path
  is silent. `expires_in` now accepts a JSON number or a numeric string in both
  `agentforce/auth.rs` and `exchange/publish.rs`; a blank string falls back to the
  conservative default lifetime instead of failing the exchange.
- **Long agent turns no longer fail with a spurious `504`.** The PDK WASM HTTP client applies
  a 10-second default timeout to any request that does not set one explicitly. Because the
  Agentforce calls never overrode it, any turn longer than ~10s (for example, multi-document
  RAG) returned a JSON-RPC `-32603` wrapping an upstream `504`. All four Agentforce calls now
  apply the configured `agentforceRequestTimeoutSeconds` (default 180s).
- **`make build` no longer risks publishing a stale policy schema.** `build-asset-files` could
  leave a stale nested `definition/target/definition/gcl.yaml` that was then copied over the
  freshly generated flat copy, so `make publish` shipped the previous schema. The generated
  `definition/target` tree is now wiped before regeneration.

### Documentation

- Documented `agentforceRequestTimeoutSeconds` in `definition/home.md` and `README.md`,
  including two troubleshooting rows that distinguish a policy-side timeout
  (`-32603` with `data.reason = agentforce_transport_error`) from a gateway-side timeout
  (a bare `504`).
- Added a "before you build or deploy" callout to `README.md` reminding operators to replace
  the placeholder Salesforce and Anypoint config values, and to set the agent card fields
  (`agentCardName`, `agentCardDescription`, `agentCardSkillsJson`) so the published AgentCard
  describes their own Agentforce agent.

### Security

- Replaced the hardcoded example credentials in `playground/config/api.yaml` (`consumerKey`,
  `consumerSecret`, `anypointClientId`, `anypointClientSecret`) with `****` placeholders so
  no credentials are committed to the repository.

[Unreleased]: https://github.com/P4A-Policies-for-Agents/a2a-policy-for-agentforce/commits/main
