# Rust WebSocket System Blueprint (Framework-Level)

## Overview

This document defines the full design of Forge's **real-time WebSocket system** — covering what's built, what's missing, and phased improvements for production-grade real-time features.

Goal:

> Provide a channel-based real-time system with private/presence channels, auth callbacks, client events, rate limiting, heartbeat, message acknowledgment, and job-integrated broadcasting — with DX comparable to Laravel Echo/Reverb.

---

# Current State

**Status: Core complete — needs production hardening**

### What's Built
- Channel-based pub/sub (Redis + memory backends)
- Rooms within channels
- WebSocketPublisher for broadcasting from HTTP handlers/jobs
- Presence channels (join/leave tracking via Redis sets)
- Auth guards per channel (token-based, cached per connection)
- Permission checks per channel
- Multi-instance via Redis pub/sub
- Comprehensive diagnostics (connection/subscription/message counts)

### What's Missing (Priority-Ordered)

| Feature | Priority | Impact |
|---------|----------|--------|
| Heartbeat/ping-pong | **Critical** | Dead connections hang for TCP timeout |
| Per-connection rate limiting | **Critical** | No flood protection |
| Channel authorization callbacks | **Critical** | Can't do dynamic per-subscription access control |
| Private channels | **Critical** | Can't build user-scoped real-time features |
| Client events (typing, whisper) | **High** | Can't build collaborative features |
| Presence change events (join/leave broadcast) | **High** | Others don't know when someone joins |
| Max connections per user | **High** | No multi-device abuse prevention |
| Force disconnect API | **High** | Can't kick banned users |
| Channel lifecycle hooks (on_join, on_leave) | **Medium** | Can't auto-send "X joined" messages |
| Message acknowledgment | **Medium** | No delivery confirmation |
| Connection recovery | **Low** | Mobile reconnect loses state |
| Message history/replay | **Low** | New subscribers miss prior messages |
| Binary frame support | **Low** | Text-only currently |

---

# Phase 1: Critical — Production Safety

## 1.1 Heartbeat / Ping-Pong

**Current:** Ping/Pong frames are silently ignored (line 307 in websocket.rs).
**Fix:** Server sends periodic Ping frames. If no Pong received within timeout, disconnect.

### Config

```toml
[websocket]
heartbeat_interval_seconds = 30
heartbeat_timeout_seconds = 10
```

### Internal Design

- Server spawns a heartbeat task per connection
- Every `heartbeat_interval`, send a Ping frame
- If no Pong received within `heartbeat_timeout`, close the connection
- Track `last_pong_at` on ConnectionState

### Consumer DX

No consumer action needed — automatic.

---

## 1.2 Per-Connection Rate Limiting

**Current:** Zero protection — client can flood with unlimited messages.
**Fix:** Track message count per connection per window. Drop + warn if exceeded.

### Config

```toml
[websocket]
max_messages_per_second = 50
```

### Internal Design

- Per-connection atomic counter + window bucket
- In `process_client_message`, check counter before processing
- If exceeded, send error event and optionally disconnect
- Uses in-memory counter (no Redis needed — per-connection, per-instance)

---

## 1.3 Channel Authorization Callbacks

**Current:** Only static guard + permission checks.
**Fix:** Allow dynamic per-subscription authorization via callback.

### Consumer DX

```rust
registrar.channel_with_options(
    TEAM_CHAT,
    handle_chat,
    WebSocketChannelOptions::new()
        .guard(AuthGuard::Api)
        .authorize(|ctx, channel, room| async move {
            // Dynamic check: is user a member of this team?
            let team_id = room.ok_or(Error::forbidden("room required"))?;
            let is_member = TeamMember::query()
                .where_(TeamMember::USER_ID.eq(&ctx.actor().unwrap().id))
                .where_(TeamMember::TEAM_ID.eq(team_id))
                .count(ctx.app()).await? > 0;
            if is_member { Ok(()) } else { Err(Error::forbidden("not a team member")) }
        }),
);
```

### Internal Design

- Add `authorize: Option<AuthorizeCallback>` to `WebSocketChannelOptions`
- Type: `Box<dyn Fn(&WebSocketContext, &ChannelId, Option<&str>) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>`
- Called after guard/permission checks, before subscription is confirmed
- If callback returns Err, send ERROR_EVENT and reject subscription

---

## 1.4 Private Channels

**Current:** No concept of user-scoped channels.
**Fix:** Convention-based private channels where only the channel owner can subscribe.

### Consumer DX

```rust
// Register private channel pattern
registrar.private_channel("user", |ctx, user_id| async move {
    // Automatically checks actor.id == user_id
    Ok(())
});

// Client subscribes to: private-user.{user_id}
// Only the user with that ID can subscribe
```

### Internal Design

- Channel name prefix: `private-{scope}.{id}` (e.g., `private-user.123`)
- On subscribe, extract the ID from the channel name
- Verify `actor.id == extracted_id` (for simple cases)
- Or use custom authorize callback for complex cases
- Private channels don't need explicit registration if auto-auth is sufficient

---

## 1.5 Max Connections Per User

### Config

```toml
[websocket]
max_connections_per_user = 5
```

### Internal Design

- Track `user_id → Set<connection_id>` in ConnectionHub
- On new connection auth, check count
- If exceeded, reject or disconnect oldest

---

## 1.6 Force Disconnect API

### Consumer DX

```rust
// From HTTP handler or job:
app.websocket()?.disconnect_user(&user_id).await?;
app.websocket()?.disconnect_connection(connection_id).await?;
```

### Internal Design

- `WebSocketPublisher::disconnect_user()` publishes a special disconnect command via pub/sub
- Each instance checks local connections and closes matching ones

---

# Phase 2: High — Interactive Features

## 2.1 Client Events

Allow clients to send events that are relayed to other subscribers (not server-originated).

### Protocol

```json
// Client sends:
{"action": "client_event", "channel": "chat", "event": "typing", "payload": {"user": "Alice"}}

// Server relays to all OTHER subscribers of "chat" (not back to sender)
```

### Consumer DX

```rust
registrar.channel_with_options(
    CHAT,
    handle_chat,
    WebSocketChannelOptions::new()
        .allow_client_events(true)  // enable relay
        .client_event_prefix("client-")  // only events starting with "client-" are relayed
);
```

---

## 2.2 Presence Change Events

When a user joins or leaves a presence channel, broadcast the change to all subscribers.

### Auto-broadcast

```json
// On join, server sends to all subscribers:
{"channel": "chat", "event": "presence:join", "payload": {"actor_id": "user-1", "joined_at": 1234567890}}

// On leave:
{"channel": "chat", "event": "presence:leave", "payload": {"actor_id": "user-1"}}
```

---

## 2.3 Channel Lifecycle Hooks

```rust
registrar.channel_with_options(
    CHAT,
    handle_message,
    WebSocketChannelOptions::new()
        .on_join(|ctx| async move {
            ctx.publish("system", json!({"message": format!("{} joined", ctx.actor().unwrap().id)})).await
        })
        .on_leave(|ctx| async move {
            ctx.publish("system", json!({"message": format!("{} left", ctx.actor().unwrap().id)})).await
        }),
);
```

---

# Phase 3: Medium — Reliability

## 3.1 Message Acknowledgment

Client can request delivery confirmation for important messages.

### Protocol

```json
// Client sends with ack_id:
{"action": "message", "channel": "orders", "payload": {...}, "ack_id": "abc123"}

// Server responds after handler succeeds:
{"channel": "system", "event": "ack", "payload": {"ack_id": "abc123", "status": "ok"}}
```

---

## 3.2 Connection Recovery

On reconnect, client can resume from last received message.

### Design

- Server assigns a `session_id` on connection
- Messages include monotonic `sequence` numbers per channel
- Client sends `resume_from: {session_id, last_sequence}` on reconnect
- Server replays missed messages from a short Redis Stream buffer

---

# Implementation Order

| Phase | Features | Status |
|-------|----------|--------|
| 1.1 | Heartbeat/ping-pong | ✅ Done — WriterCommand enum, ping task, pong tracking, stale close |
| 1.2 | Per-connection rate limiting | ✅ Done — per-connection counter, 1s window, error on exceed |
| 1.3 | Channel authorization callbacks | ✅ Done — AuthorizeCallback, wired into Subscribe flow |
| 1.4 | Private channels | Design ready — uses authorize callback |
| 1.5 | Max connections per user | ✅ Done — user→connection tracking, limit check on auth |
| 1.6 | Force disconnect API | ✅ Done — hub + pub/sub command handling |
| 2.1 | Client events | ✅ Done — ClientAction::ClientEvent, broadcast_except (relay to others) |
| 2.2 | Presence change events | ✅ Done — auto-broadcast presence:join / presence:leave |
| 2.3 | Channel lifecycle hooks | ✅ Done — .on_join() / .on_leave() callbacks |
| 3.1 | Message acknowledgment | ✅ Done — `ack_id` on ClientMessage, ACK_EVENT after handler |
| 3.2 | Connection recovery | ✅ Done — message buffer (Redis LPUSH+LTRIM), `.replay(count)` on subscribe |

---

# Security Checklist

| Concern | Current | Target |
|---------|---------|--------|
| Auth per channel | ✅ Guard + permissions | + authorization callbacks |
| Token revocation | ⚠️ Cached, no re-validation | Add cache TTL or re-validate periodically |
| Rate limiting | ❌ None | Per-connection message rate limit |
| Connection limits | ❌ None | Max per user |
| Force disconnect | ❌ None | API for banning/kicking |
| CORS | ⚠️ HTTP-layer only | Document WS-specific guidance |

---

# Config (Complete)

```toml
[websocket]
host = "127.0.0.1"
port = 3010
path = "/ws"

# Phase 1
heartbeat_interval_seconds = 30
heartbeat_timeout_seconds = 10
max_messages_per_second = 50
max_connections_per_user = 5
```

---

# Assumptions

- WebSocket runs on a separate port from HTTP (existing design)
- Redis pub/sub for multi-instance broadcasting (existing)
- Presence tracked via Redis sets (existing)
- Auth tokens validated once per connection, cached (existing)
- Private channel convention: `private-{scope}.{id}` prefix
- Client events are opt-in per channel (not default)
- Message acknowledgment is opt-in per message (not default)
- Connection recovery uses Redis Streams for short-term buffer

---

# One-Line Goal

> A Forge WebSocket channel should support private/presence/public modes with dynamic authorization, client events, lifecycle hooks, rate limiting, and heartbeat — all configurable per-channel with the same DX quality as the HTTP routing system.
