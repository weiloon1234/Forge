# WebSocket Guide

Channel-based real-time communication with presence tracking, rooms, auth, and server-side broadcasting.

---

## Quick Start

```rust
const CHAT: ChannelId = ChannelId::new("chat");
const MESSAGE: ChannelEventId = ChannelEventId::new("message");

struct ChatHandler;

#[async_trait]
impl ChannelHandler for ChatHandler {
    async fn handle(&self, ctx: WebSocketContext, payload: Value) -> Result<()> {
        // Broadcast to all subscribers
        ctx.publish(MESSAGE, &payload).await
    }
}

fn ws_routes(r: &mut WebSocketRegistrar) -> Result<()> {
    r.channel(CHAT, ChatHandler)?;
    Ok(())
}
```

Register and run:

```rust
App::builder()
    .register_websocket_routes(ws_routes)
    .run_websocket()?;
```

Clients connect to `ws://host:3010/ws` and subscribe to channels by sending:

```json
{ "action": "Subscribe", "channel": "chat" }
```

---

## Channels

### Basic Channel

```rust
r.channel(ChannelId::new("notifications"), NotificationHandler)?;
```

### Channel with Options

```rust
r.channel_with_options(
    ChannelId::new("orders"),
    OrderHandler,
    WebSocketChannelOptions::new()
        .guard(Guard::User)                         // require auth
        .permission(Permission::OrdersView)          // require permission
        .presence(true)                              // track who's connected
        .allow_client_events(true)                   // clients can relay to other clients
        .replay(10)                                  // send last 10 messages to new subscribers
        .authorize(|ctx, channel, room| async move {
            // Dynamic auth — e.g., check if user owns this order
            Ok(())
        })
        .on_join(|ctx| async move {
            tracing::info!(user = ?ctx.actor(), "joined orders channel");
            Ok(())
        })
        .on_leave(|ctx| async move {
            tracing::info!(user = ?ctx.actor(), "left orders channel");
            Ok(())
        }),
)?;
```

### Channel Options

| Method | What it does |
|--------|-------------|
| `.guard(Guard::User)` | Require auth guard for subscription |
| `.permission(Permission::X)` | Require specific permission |
| `.permissions([...])` | Require all listed permissions |
| `.authorize(async fn)` | Custom async auth check after guard/permission |
| `.presence(true)` | Enable join/leave tracking |
| `.allow_client_events(true)` | Allow clients to relay events to other clients |
| `.replay(N)` | Buffer last N messages, send to new subscribers |
| `.on_join(async fn)` | Callback when a user subscribes |
| `.on_leave(async fn)` | Callback when a user unsubscribes |

---

## Handling Messages

### ChannelHandler Trait

```rust
struct OrderHandler;

#[async_trait]
impl ChannelHandler for OrderHandler {
    async fn handle(&self, ctx: WebSocketContext, payload: Value) -> Result<()> {
        let user = ctx.resolve_actor::<User>().await?
            .ok_or_else(|| Error::message("user not found"))?;

        let order_id = payload.get("order_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::message("missing order_id"))?;

        // Process the message...

        // Broadcast response to all subscribers
        ctx.publish(ChannelEventId::new("order_updated"), json!({
            "order_id": order_id,
            "updated_by": user.name,
        })).await
    }
}
```

### WebSocketContext

Available in every handler:

```rust
ctx.app()                  // → &AppContext (full framework access)
ctx.connection_id()        // → u64 (unique per connection)
ctx.actor()                // → Option<&Actor> (authenticated user)
ctx.channel()              // → &ChannelId
ctx.room()                 // → Option<&str>

// Resolve to database model
let user = ctx.resolve_actor::<User>().await?;

// Publish to this channel
ctx.publish(EVENT_ID, json!({ "data": "value" })).await?;

// Presence
let members = ctx.presence_members().await?;  // Vec<PresenceInfo>
let count = ctx.presence_count().await?;      // usize
```

---

## Rooms

Rooms are subdivisions within a channel. A client subscribes to a channel + room combination:

```json
{ "action": "Subscribe", "channel": "chat", "room": "room:42" }
```

Publish to a specific room:

```rust
// Only subscribers in room "room:42" receive this
app.websocket()?.publish(
    ChannelId::new("chat"),
    ChannelEventId::new("message"),
    Some("room:42"),
    json!({ "text": "hello room 42" }),
).await?;
```

Publish to the whole channel (all rooms):

```rust
app.websocket()?.publish(
    ChannelId::new("chat"),
    ChannelEventId::new("announcement"),
    None,  // no room = broadcast to all
    json!({ "text": "server maintenance in 5 minutes" }),
).await?;
```

---

## Presence

Track who's connected to a channel in real time.

Enable on channel:

```rust
WebSocketChannelOptions::new().presence(true)
```

Query in handler:

```rust
async fn handle(&self, ctx: WebSocketContext, _payload: Value) -> Result<()> {
    let members = ctx.presence_members().await?;
    for member in &members {
        // member.actor_id, member.channel, member.joined_at
    }

    let online_count = ctx.presence_count().await?;

    // Broadcast current member list
    ctx.publish(ChannelEventId::new("presence_update"), json!({
        "members": members.iter().map(|m| &m.actor_id).collect::<Vec<_>>(),
        "count": online_count,
    })).await
}
```

**Automatic events** (sent by the framework, not your handler):

| Event | When | Payload |
|-------|------|---------|
| `presence:join` | User subscribes to channel | `{ "actor_id": "..." }` |
| `presence:leave` | User unsubscribes or disconnects | `{ "actor_id": "..." }` |

---

## Broadcasting from HTTP Handlers / Jobs

Publish WebSocket messages from anywhere — not just inside channel handlers:

```rust
// In an HTTP handler
async fn update_order(
    State(app): State<AppContext>,
    Path(order_id): Path<String>,
) -> Result<impl IntoResponse> {
    // ... update order in database ...

    // Broadcast to WebSocket subscribers
    app.websocket()?.publish(
        ChannelId::new("orders"),
        ChannelEventId::new("updated"),
        Some(&format!("order:{order_id}")),
        json!({ "order_id": order_id, "status": "shipped" }),
    ).await?;

    Ok(Json(json!({ "ok": true })))
}

// In a background job
impl Job for ProcessOrderJob {
    async fn handle(&self, ctx: JobContext) -> Result<()> {
        // ... process order ...

        ctx.app().websocket()?.publish(
            ChannelId::new("orders"),
            ChannelEventId::new("processed"),
            None,
            json!({ "order_id": self.order_id }),
        ).await
    }
}
```

### Force Disconnect

Kick a user from all WebSocket connections (e.g., after ban):

```rust
app.websocket()?.disconnect_user(&user_id).await?;
```

Works across distributed instances via Redis pub/sub.

---

## Client Protocol

Clients communicate via JSON frames over WebSocket:

### Subscribe

```json
{ "action": "Subscribe", "channel": "chat" }
{ "action": "Subscribe", "channel": "chat", "room": "room:42" }
```

Server responds:

```json
{ "channel": "chat", "event": "subscribed" }
```

### Unsubscribe

```json
{ "action": "Unsubscribe", "channel": "chat" }
```

### Send Message

```json
{
    "action": "Message",
    "channel": "chat",
    "event": "message",
    "payload": { "text": "hello" },
    "ack_id": "optional-client-id"
}
```

If `ack_id` is provided, server responds with:

```json
{ "channel": "system", "event": "ack", "payload": { "ack_id": "optional-client-id" } }
```

### Client Events (peer-to-peer relay)

When `allow_client_events(true)` is set:

```json
{
    "action": "ClientEvent",
    "channel": "chat",
    "event": "typing",
    "payload": { "user": "Alice" }
}
```

Relayed to all other subscribers (not back to sender).

---

## System Events

The framework automatically sends these events:

| Constant | Event | Description |
|----------|-------|-------------|
| `SUBSCRIBED_EVENT` | `subscribed` | Subscription confirmed |
| `UNSUBSCRIBED_EVENT` | `unsubscribed` | Unsubscription confirmed |
| `PRESENCE_JOIN_EVENT` | `presence:join` | User joined (presence channels) |
| `PRESENCE_LEAVE_EVENT` | `presence:leave` | User left (presence channels) |
| `ERROR_EVENT` | `error` | Error occurred |
| `ACK_EVENT` | `ack` | Message acknowledged |

All system events are sent on the `system` channel.

---

## Config

```toml
# config/websocket.toml
[websocket]
host = "127.0.0.1"
port = 3010
path = "/ws"
heartbeat_interval_seconds = 30       # server pings client
heartbeat_timeout_seconds = 10        # disconnect if no pong
max_messages_per_second = 50          # per-connection flood protection
max_connections_per_user = 5          # multi-device limit
```

---

## Event Integration

Automatically broadcast domain events to WebSocket:

```rust
// In ServiceProvider — listen for events and broadcast
registrar.listen_event::<OrderPlaced, _>(
    publish_websocket(|event: &OrderPlaced| ServerMessage {
        channel: ChannelId::new("orders"),
        event: ChannelEventId::new("placed"),
        room: None,
        payload: json!({ "order_id": event.order_id }),
    })
)?;
```

See [Background Processing Guide](background-processing.md) for event→websocket helpers.

---

## Observability

Forge exposes read-only JSON endpoints under the observability base path (default `/_forge`) for inspecting WebSocket state from ops tooling or custom admin apps. All endpoints honor the same `ObservabilityOptions` access scope as the rest of the dashboard — gate them behind a guard and permission for production use.

### Endpoints

| Route                                 | Purpose                                      |
| ------------------------------------- | -------------------------------------------- |
| `GET /_forge/ws/channels`             | List all registered channels and their options |
| `GET /_forge/ws/presence/:channel`    | Live presence members for a presence channel |
| `GET /_forge/ws/history/:channel`     | Last up-to-50 buffered messages (metadata only by default) |
| `GET /_forge/ws/stats`                | Global + per-channel counters                |

#### Example: list registered channels

```bash
curl -s http://localhost:3000/_forge/ws/channels | jq
```

```json
{
  "channels": [
    {
      "id": "chat",
      "presence": true,
      "replay_count": 10,
      "allow_client_events": false,
      "requires_auth": true,
      "guard": "api",
      "permissions": ["chat:read"]
    }
  ]
}
```

#### Example: inspect presence

```bash
curl -s http://localhost:3000/_forge/ws/presence/chat | jq
```

```json
{
  "channel": "chat",
  "count": 3,
  "members": [
    { "actor_id": "user_1", "joined_at": 1713456789 }
  ]
}
```

#### Example: peek recent history (metadata only)

```bash
curl -s "http://localhost:3000/_forge/ws/history/chat?limit=10" | jq
```

Each entry includes `{ channel, event, room, payload_size_bytes }`. The raw `payload` is **not** included by default.

History lists are capped at 50 entries per channel. Each publish also refreshes a TTL on the history key (default 7 days, configured via `websocket.history_ttl_seconds`), so channels that go silent are auto-reaped by Redis — no manual cleanup scheduler needed. Set `history_ttl_seconds = 0` to disable and retain history indefinitely.

### Including payloads in history

If you need to see message bodies (e.g., in staging or internal tooling), opt in via config:

```toml
[observability.websocket]
include_payloads = true
```

Or via environment:

```
OBSERVABILITY__WEBSOCKET__INCLUDE_PAYLOADS=true
```

When enabled, `/ws/history/:channel` returns the full `ServerMessage.payload` for each buffered entry. Use this with care — payloads may contain personal or sensitive data.

### Per-channel stats

`GET /_forge/ws/stats` pairs the existing global counters with a per-channel breakdown:

```json
{
  "global": {
    "active_connections": 40,
    "active_subscriptions": 85,
    "inbound_messages_total": 12300,
    "outbound_messages_total": 45600
  },
  "channels": [
    {
      "id": "chat",
      "active_subscriptions": 20,
      "subscriptions_total": 200,
      "unsubscribes_total": 180,
      "inbound_messages_total": 5000,
      "outbound_messages_total": 20000
    }
  ]
}
```

Registered channels with no traffic appear with zero counters. Counters are per-process, matching the semantics of the existing global counters — aggregate across instances in your metrics backend if needed.

The same series are also emitted in Prometheus format on `/_forge/metrics`, labelled by `channel`:

```
forge_websocket_subscriptions_total{channel="chat"} 200
forge_websocket_active_subscriptions{channel="chat"} 20
forge_websocket_channel_messages_total{channel="chat",direction="inbound"} 5000
forge_websocket_channel_messages_total{channel="chat",direction="outbound"} 20000
```

### What these endpoints intentionally don't do

- **No admin actions.** Broadcast, force-disconnect, and history purge are deliberately not exposed. Build those into your app code where the authorization story is yours to own.
- **No bundled UI.** Every endpoint returns JSON; wire it into whatever dashboard you already run.
- **No per-connection list.** Per-node connection registries are confusing in multi-instance deployments. Use presence to see who is subscribed.
