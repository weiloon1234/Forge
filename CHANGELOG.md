# Changelog

All notable changes to this project will be documented in this file.

The format is inspired by Keep a Changelog, adapted for Forge's pre-`1.0` releases.

## [Unreleased]

### Added

- Release infrastructure: GitHub Actions CI, release-readiness workflow, release checklist, and local package dry-run verification.
- Consumer documentation: root README, contributing guide, and a first-class plugin example.
- WebSocket observability dashboard endpoints: `GET /_forge/ws/channels`, `GET /_forge/ws/presence/:channel`, `GET /_forge/ws/history/:channel`, and `GET /_forge/ws/stats`. History payloads are redacted by default; set `observability.websocket.include_payloads = true` to include them.
- Per-channel WebSocket Prometheus series on `/_forge/metrics` (`forge_websocket_subscriptions_total{channel=...}`, `forge_websocket_active_subscriptions{channel=...}`, `forge_websocket_channel_messages_total{channel=...,direction=...}`).
- HTTP request latency histograms on `/_forge/runtime` and `/_forge/metrics` via `forge_http_request_duration_ms_bucket`, `_sum`, and `_count`, which can be used to compute p50/p95/p99 in Prometheus-compatible backends.
- `AppContext::websocket_channels()` accessor returning the registered channel registry.
- `WebSocketChannelDescriptor` and `WebSocketChannelRegistry` public types exposing registered WebSocket channels.
- Configurable TTL on WebSocket replay history (`websocket.history_ttl_seconds`, default 7 days). Every publish refreshes the TTL on `ws:history:<channel>`, so active channels never expire; channels idle past the window are auto-reaped by Redis. Set to `0` to disable.
- WebSocket hardening config: `websocket.outbound_buffer_size`, `websocket.allowed_origins`, and `websocket.history_buffer_size`.
- WebSocket protocol/lifecycle acceptance coverage for raw JSON actions, subscription enforcement, room routing, client events, ack success/error, socket-close cleanup, and force-disconnect cleanup.

### Changed

- Crate metadata is now publish-ready for the `0.1.x` line.
- Verification contract now explicitly includes both fixture families and packaging checks.
- `MaxBodySize` now also updates Axum's default extractor body limit, so JSON/Form/String extractors honor the configured Forge limit instead of staying capped at Axum's 2 MiB default.
- Framework model post-write events (`ModelCreatedEvent`, `ModelUpdatedEvent`, and `ModelDeletedEvent`) now dispatch after the active transaction commits, making event listeners safe for dependent writes and queued onboarding jobs that need the committed row to be visible.
- `WebSocketRuntimeSnapshot` now includes a `channels: Vec<WebSocketChannelSnapshot>` field in addition to the existing global counters.
- `WebSocketKernel::new` no longer takes a `Vec<WebSocketRouteRegistrar>`; registered channels are built once during `AppBuilder::bootstrap()` and resolved from the DI container. Direct callers of `WebSocketKernel::new` must drop the routes argument.
- `RuntimeDiagnostics` inbound-message recording at the kernel now runs after `serde_json::from_str` parses the client message (so only parseable messages are counted). Malformed frames no longer increment `inbound_messages_total`.
- WebSocket wire actions are documented as canonical `snake_case`; legacy PascalCase action aliases remain accepted for compatibility.
- WebSocket room routing is now explicit: channel-wide publishes reach all subscribers, while room publishes reach only exact room subscribers.
- WebSocket `on_leave` hooks and `presence:leave` now run for unsubscribe, socket close, heartbeat timeout, and force disconnect.
- WebSocket channel callbacks now receive owned context/channel/room values, which makes async closures easier to use safely.

### Breaking

- WebSocket `message` and `client_event` frames now require an active matching channel/room subscription before handlers or client-event relay run.
