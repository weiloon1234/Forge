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

### Changed

- Crate metadata is now publish-ready for the `0.1.x` line.
- Verification contract now explicitly includes both fixture families and packaging checks.
- `WebSocketRuntimeSnapshot` now includes a `channels: Vec<WebSocketChannelSnapshot>` field in addition to the existing global counters.
- `WebSocketKernel::new` no longer takes a `Vec<WebSocketRouteRegistrar>`; registered channels are built once during `AppBuilder::bootstrap()` and resolved from the DI container. Direct callers of `WebSocketKernel::new` must drop the routes argument.
- `RuntimeDiagnostics` inbound-message recording at the kernel now runs after `serde_json::from_str` parses the client message (so only parseable messages are counted). Malformed frames no longer increment `inbound_messages_total`.
