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
