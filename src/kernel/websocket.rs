use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, RwLock};

use crate::auth::{Actor, AuthError, AuthErrorCode};
use crate::config::WebSocketConfig;
use crate::foundation::{AppContext, Error, Result};
use crate::logging::{AuthOutcome, RuntimeDiagnostics, WebSocketConnectionState};
use crate::support::runtime::RuntimeBackend;
use crate::support::{ChannelEventId, ChannelId, GuardId};
use crate::websocket::{
    presence_key, presence_member_value, ClientAction, ClientMessage, RegisteredChannel,
    ServerMessage, WebSocketContext, ACK_EVENT, ERROR_EVENT, PRESENCE_JOIN_EVENT,
    PRESENCE_LEAVE_EVENT, SUBSCRIBED_EVENT, SYSTEM_CHANNEL, UNSUBSCRIBED_EVENT,
};

pub struct WebSocketKernel {
    app: AppContext,
}

impl WebSocketKernel {
    pub fn new(app: AppContext) -> Self {
        Self { app }
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }

    pub async fn bind(self) -> Result<BoundWebSocketServer> {
        let websocket = self.app.config().websocket()?;
        let addr = format!("{}:{}", websocket.host, websocket.port);
        let listener = TcpListener::bind(addr).await.map_err(Error::other)?;
        let local_addr = listener.local_addr().map_err(Error::other)?;
        let router = self.build_router().await?;

        Ok(BoundWebSocketServer {
            listener,
            router,
            local_addr,
        })
    }

    pub async fn serve(self) -> Result<()> {
        self.bind().await?.serve().await
    }

    async fn build_router(&self) -> Result<axum::Router> {
        let ws_config = self.app.config().websocket()?;
        let registry = self
            .app
            .container()
            .resolve::<crate::websocket::WebSocketChannelRegistry>()?;
        let registered_channels: Vec<RegisteredChannel> = registry.registered_channels().to_vec();
        let backend = RuntimeBackend::from_config(self.app.config())?;
        let state =
            WebSocketServerState::new(self.app.clone(), registered_channels, backend, ws_config);
        state.start_pubsub().await?;

        Ok(axum::Router::new()
            .route(&state.ws_config.path, get(websocket_handler))
            .with_state(state))
    }
}

pub struct BoundWebSocketServer {
    listener: TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
}

impl BoundWebSocketServer {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn serve(self) -> Result<()> {
        axum::serve(self.listener, self.router)
            .await
            .map_err(Error::other)
    }
}

/// Commands sent to the per-connection writer task.
enum WriterCommand {
    Json(ServerMessage),
    Ping,
    Close,
}

#[derive(Clone)]
struct WebSocketServerState {
    app: AppContext,
    channels: Arc<HashMap<ChannelId, RegisteredChannel>>,
    hub: ConnectionHub,
    backend: RuntimeBackend,
    ws_config: WebSocketConfig,
}

impl WebSocketServerState {
    fn new(
        app: AppContext,
        channels: Vec<RegisteredChannel>,
        backend: RuntimeBackend,
        ws_config: WebSocketConfig,
    ) -> Self {
        let map = channels
            .into_iter()
            .map(|channel| (channel.id.clone(), channel))
            .collect::<HashMap<_, _>>();
        let diagnostics = app.diagnostics().ok();
        Self {
            app,
            channels: Arc::new(map),
            hub: ConnectionHub::new(diagnostics),
            backend,
            ws_config,
        }
    }

    async fn start_pubsub(&self) -> Result<()> {
        if self.channels.is_empty() {
            return Ok(());
        }

        let backend = RuntimeBackend::from_config(self.app.config())?;
        let mut topics = self
            .channels
            .keys()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>();

        // Subscribe to the system disconnect topic for force-disconnect support.
        topics.push("__system:disconnect".to_string());

        let hub = self.hub.clone();
        let ws_backend = self.backend.clone();
        tokio::spawn(async move {
            let mut subscription = match backend.subscribe_ws(&topics).await {
                Ok(subscription) => subscription,
                Err(error) => {
                    tracing::error!("forge websocket pubsub startup failed: {error}");
                    return;
                }
            };

            while let Some(message) = subscription.recv().await {
                // Handle force-disconnect commands on the system topic.
                if message.topic == "__system:disconnect" {
                    #[derive(serde::Deserialize)]
                    struct DisconnectCommand {
                        actor_id: String,
                    }
                    if let Ok(cmd) = serde_json::from_str::<DisconnectCommand>(&message.payload) {
                        let entries = hub.disconnect_by_actor(&cmd.actor_id).await;
                        for entry in entries {
                            let _ = ws_backend.srem(&entry.key, &entry.member_value).await;
                        }
                    } else {
                        tracing::error!(
                            "forge websocket pubsub: invalid disconnect command payload"
                        );
                    }
                    continue;
                }

                let envelope = match serde_json::from_str::<ServerMessage>(&message.payload) {
                    Ok(envelope) => envelope,
                    Err(error) => {
                        tracing::error!("forge websocket pubsub decode failed: {error}");
                        continue;
                    }
                };
                hub.broadcast(&envelope).await;
            }
        });

        Ok(())
    }

    async fn capture_identity(&self, headers: &HeaderMap) -> ConnectionIdentity {
        let Ok(auth) = self.app.auth() else {
            return ConnectionIdentity {
                bearer_token: None,
                session_id: None,
                auth_error: Some(AuthError::internal("auth manager is not available")),
                client_ip: None,
            };
        };

        // Try bearer token first (Authorization header)
        if headers.contains_key(axum::http::header::AUTHORIZATION) {
            return match auth.extract_token(headers) {
                Ok(token) => ConnectionIdentity {
                    bearer_token: Some(token),
                    session_id: None,
                    auth_error: None,
                    client_ip: None,
                },
                Err(error) => ConnectionIdentity {
                    bearer_token: None,
                    session_id: None,
                    auth_error: Some(error),
                    client_ip: None,
                },
            };
        }

        // Fall back to session cookie
        if let Ok(sessions) = self.app.sessions() {
            if let Some(sid) = sessions.extract_session_id(headers) {
                return ConnectionIdentity {
                    bearer_token: None,
                    session_id: Some(sid),
                    auth_error: None,
                    client_ip: None,
                };
            }
        }

        ConnectionIdentity::default()
    }

    async fn authorize_channel(
        &self,
        connection_id: u64,
        channel: &RegisteredChannel,
    ) -> std::result::Result<Option<Actor>, AuthError> {
        if !channel.options.requires_auth() {
            return Ok(None);
        }

        let auth = match self.app.auth() {
            Ok(auth) => auth,
            Err(error) => {
                self.record_auth_outcome(AuthOutcome::Error);
                return Err(AuthError::internal(error.to_string()));
            }
        };
        let authorizer = match self.app.authorizer() {
            Ok(authorizer) => authorizer,
            Err(error) => {
                self.record_auth_outcome(AuthOutcome::Error);
                return Err(AuthError::internal(error.to_string()));
            }
        };
        let guard_id = channel
            .options
            .guard_id()
            .cloned()
            .unwrap_or_else(|| auth.default_guard().clone());

        if let Some(actor) = self.hub.cached_actor(connection_id, &guard_id).await? {
            let permissions = channel.options.permissions_set();
            if let Err(error) = authorizer.authorize_permissions(&actor, &permissions).await {
                self.record_auth_outcome(auth_outcome_from_error(&error));
                return Err(error);
            }
            self.record_auth_outcome(AuthOutcome::Success);
            return Ok(Some(actor));
        }

        let identity = self.hub.identity(connection_id).await?;
        if let Some(error) = identity.auth_error {
            self.record_auth_outcome(auth_outcome_from_error(&error));
            return Err(error);
        }

        // Resolve actor from either bearer token or session cookie
        let actor = if let Some(session_id) = identity.session_id {
            let sessions = self
                .app
                .sessions()
                .map_err(|e| AuthError::internal(e.to_string()))
                .inspect_err(|e| self.record_auth_outcome(auth_outcome_from_error(e)))?;
            match sessions.validate(&session_id).await {
                Ok(Some(actor)) => actor.with_guard(guard_id.clone()),
                Ok(None) => {
                    let error = AuthError::unauthorized_code(AuthErrorCode::InvalidSession);
                    self.record_auth_outcome(auth_outcome_from_error(&error));
                    return Err(error);
                }
                Err(e) => {
                    let error = AuthError::internal(e.to_string());
                    self.record_auth_outcome(auth_outcome_from_error(&error));
                    return Err(error);
                }
            }
        } else if let Some(token) = identity.bearer_token {
            match auth.authenticate_token(&token, Some(&guard_id)).await {
                Ok(actor) => actor,
                Err(error) => {
                    self.record_auth_outcome(auth_outcome_from_error(&error));
                    return Err(error);
                }
            }
        } else {
            let error = AuthError::unauthorized_code(AuthErrorCode::MissingAuthCredentials);
            self.record_auth_outcome(auth_outcome_from_error(&error));
            return Err(error);
        };
        let permissions = channel.options.permissions_set();
        if let Err(error) = authorizer.authorize_permissions(&actor, &permissions).await {
            self.record_auth_outcome(auth_outcome_from_error(&error));
            return Err(error);
        }
        self.hub
            .cache_actor(
                connection_id,
                actor.clone(),
                self.ws_config.max_connections_per_user,
            )
            .await?;
        self.record_auth_outcome(AuthOutcome::Success);
        Ok(Some(actor))
    }

    fn record_auth_outcome(&self, outcome: AuthOutcome) {
        if let Ok(diagnostics) = self.app.diagnostics() {
            diagnostics.record_auth_outcome(outcome);
        }
    }
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    uri: axum::http::Uri,
    headers: HeaderMap,
    State(state): State<WebSocketServerState>,
) -> Response {
    // Support token via query param (?token=xxx) for browser WebSocket connections
    // which cannot set custom headers.
    let mut headers = headers;
    if !headers.contains_key(axum::http::header::AUTHORIZATION) {
        if let Some(query) = uri.query() {
            for pair in query.split('&') {
                if let Some(token) = pair.strip_prefix("token=") {
                    if let Ok(value) = format!("Bearer {token}").parse() {
                        headers.insert(axum::http::header::AUTHORIZATION, value);
                    }
                    break;
                }
            }
        }
    }

    let mut identity = state.capture_identity(&headers).await;
    identity.client_ip = extract_client_ip_from_headers(&headers);
    ws.on_upgrade(move |socket| handle_socket(socket, state, identity))
}

fn extract_client_ip_from_headers(headers: &HeaderMap) -> Option<String> {
    let ip = crate::http::middleware::resolve_real_ip(headers, &[]);
    if ip.is_unspecified() {
        None
    } else {
        Some(ip.to_string())
    }
}

async fn handle_socket(
    socket: WebSocket,
    state: WebSocketServerState,
    identity: ConnectionIdentity,
) {
    let (connection_id, mut outbound, last_pong_at) = state.hub.register(identity).await;
    let (mut sender, mut receiver) = socket.split();

    // Writer task: serializes WriterCommands into WebSocket frames.
    let writer = tokio::spawn(async move {
        while let Some(command) = outbound.recv().await {
            match command {
                WriterCommand::Json(message) => {
                    let payload = match serde_json::to_string(&message) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    if sender.send(Message::Text(payload.into())).await.is_err() {
                        break;
                    }
                }
                WriterCommand::Ping => {
                    if sender.send(Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                }
                WriterCommand::Close => break,
            }
        }
    });

    // Heartbeat task: sends pings and closes the connection on timeout.
    let heartbeat_sender = state.hub.sender(connection_id).await;
    let heartbeat_pong = last_pong_at.clone();
    let heartbeat_interval = Duration::from_secs(state.ws_config.heartbeat_interval_seconds.max(1));
    let heartbeat_timeout = Duration::from_secs(state.ws_config.heartbeat_timeout_seconds.max(1));
    let heartbeat = tokio::spawn(async move {
        let mut interval = tokio::time::interval(heartbeat_interval);
        interval.tick().await; // skip first immediate tick
        loop {
            interval.tick().await;
            let Some(ref hb_sender) = heartbeat_sender else {
                break;
            };
            if hb_sender.send(WriterCommand::Ping).is_err() {
                break;
            }
            let elapsed = heartbeat_pong.lock().await.elapsed();
            if elapsed > heartbeat_interval + heartbeat_timeout {
                let _ = hb_sender.send(WriterCommand::Close);
                break;
            }
        }
    });

    while let Some(result) = receiver.next().await {
        let message = match result {
            Ok(message) => message,
            Err(_) => break,
        };

        match message {
            Message::Text(text) => {
                if let Err(error) =
                    process_client_message(&state, connection_id, text.to_string()).await
                {
                    let _ = state
                        .hub
                        .send(
                            connection_id,
                            WriterCommand::Json(ServerMessage {
                                channel: SYSTEM_CHANNEL,
                                event: ERROR_EVENT,
                                room: None,
                                payload: error.payload(),
                            }),
                        )
                        .await;
                }
            }
            Message::Pong(_) => {
                *last_pong_at.lock().await = tokio::time::Instant::now();
            }
            Message::Close(_) => break,
            Message::Binary(_) | Message::Ping(_) => {}
        }
    }

    let presence_entries = state.hub.unregister(connection_id).await;
    for entry in presence_entries {
        let _ = state.backend.srem(&entry.key, &entry.member_value).await;
    }
    writer.abort();
    heartbeat.abort();
}

async fn process_client_message(
    state: &WebSocketServerState,
    connection_id: u64,
    payload: String,
) -> Result<()> {
    // Per-connection rate limiting.
    if !state
        .hub
        .check_rate_limit(connection_id, state.ws_config.max_messages_per_second)
        .await
    {
        state
            .hub
            .send(
                connection_id,
                WriterCommand::Json(ServerMessage {
                    channel: SYSTEM_CHANNEL,
                    event: ERROR_EVENT,
                    room: None,
                    payload: serde_json::json!({"message": "rate limit exceeded"}),
                }),
            )
            .await
            .ok();
        return Ok(());
    }

    let message: ClientMessage = serde_json::from_str(&payload).map_err(Error::other)?;
    if let Ok(diagnostics) = state.app.diagnostics() {
        diagnostics.record_websocket_inbound_message_on(&message.channel);
    }
    let Some(channel) = state.channels.get(&message.channel) else {
        return Err(Error::message(format!(
            "websocket channel `{}` is not registered",
            message.channel
        )));
    };

    match message.action {
        ClientAction::Subscribe => {
            let actor = match state.authorize_channel(connection_id, channel).await {
                Ok(actor) => actor,
                Err(error) => {
                    state
                        .hub
                        .send(
                            connection_id,
                            WriterCommand::Json(ServerMessage {
                                channel: SYSTEM_CHANNEL,
                                event: ERROR_EVENT,
                                room: None,
                                payload: error.payload(),
                            }),
                        )
                        .await
                        .ok();
                    return Ok(());
                }
            };

            // Authorization callback (Feature 4).
            if let Some(ref authorize) = channel.options.authorize {
                let ctx = WebSocketContext::new(
                    state.app.clone(),
                    connection_id,
                    actor.clone(),
                    message.channel.clone(),
                    message.room.clone(),
                );
                if let Err(error) = authorize(&ctx, &message.channel, message.room.as_deref()).await
                {
                    state
                        .hub
                        .send(
                            connection_id,
                            WriterCommand::Json(ServerMessage {
                                channel: SYSTEM_CHANNEL,
                                event: ERROR_EVENT,
                                room: None,
                                payload: error.payload(),
                            }),
                        )
                        .await
                        .ok();
                    return Ok(());
                }
            }

            state
                .hub
                .subscribe(connection_id, &message.channel, message.room.clone())
                .await;

            // Track presence if enabled for this channel.
            if channel.options.presence {
                let actor_id = actor
                    .as_ref()
                    .map(|a| a.id.clone())
                    .unwrap_or_else(|| format!("anon:{connection_id}"));
                let now = chrono::Utc::now().timestamp();
                let key = presence_key(&message.channel);
                let member_value = presence_member_value(&actor_id, &message.channel, now);
                let _ = state.backend.sadd(&key, &member_value).await;
                state
                    .hub
                    .add_presence_entry(connection_id, PresenceEntry { key, member_value })
                    .await;

                // Broadcast presence join event to all subscribers.
                let join_msg = ServerMessage {
                    channel: message.channel.clone(),
                    event: PRESENCE_JOIN_EVENT,
                    room: message.room.clone(),
                    payload: serde_json::json!({
                        "actor_id": actor_id,
                        "joined_at": now,
                    }),
                };
                state.hub.broadcast_except(connection_id, &join_msg).await;
            }

            // Invoke on_join lifecycle hook.
            if let Some(ref on_join) = channel.options.on_join {
                let ctx = WebSocketContext::new(
                    state.app.clone(),
                    connection_id,
                    actor.clone(),
                    message.channel.clone(),
                    message.room.clone(),
                );
                if let Err(e) = on_join(&ctx).await {
                    tracing::warn!(target: "forge.websocket", error = %e, "on_join hook failed");
                }
            }

            // Replay recent messages before sending SUBSCRIBED so the client catches up.
            if channel.options.replay_count > 0 {
                let history_key = format!("ws:history:{}", message.channel);
                if let Ok(messages) = state
                    .backend
                    .lrange(&history_key, 0, channel.options.replay_count as i64 - 1)
                    .await
                {
                    // Messages are stored newest-first (LPUSH), send oldest-first.
                    for raw in messages.into_iter().rev() {
                        if let Ok(msg) = serde_json::from_str::<ServerMessage>(&raw) {
                            let _ = state
                                .hub
                                .send(connection_id, WriterCommand::Json(msg))
                                .await;
                        }
                    }
                }
            }

            state
                .hub
                .send(
                    connection_id,
                    WriterCommand::Json(ServerMessage {
                        channel: message.channel,
                        event: SUBSCRIBED_EVENT,
                        room: message.room,
                        payload: serde_json::Value::Null,
                    }),
                )
                .await?;
        }
        ClientAction::Unsubscribe => {
            // Invoke on_leave lifecycle hook.
            if let Some(ref on_leave) = channel.options.on_leave {
                let guard_id = channel
                    .options
                    .guard_id()
                    .cloned()
                    .unwrap_or_else(|| GuardId::new("default"));
                let actor = state
                    .hub
                    .cached_actor(connection_id, &guard_id)
                    .await
                    .ok()
                    .flatten();
                let ctx = WebSocketContext::new(
                    state.app.clone(),
                    connection_id,
                    actor,
                    message.channel.clone(),
                    message.room.clone(),
                );
                if let Err(e) = on_leave(&ctx).await {
                    tracing::warn!(target: "forge.websocket", error = %e, "on_leave hook failed");
                }
            }

            // Clean up presence entries for this channel before unsubscribing.
            let entries = state
                .hub
                .take_presence_entries_for_channel(connection_id, &message.channel)
                .await;
            for entry in &entries {
                let _ = state.backend.srem(&entry.key, &entry.member_value).await;
            }

            // Broadcast presence leave event if there were presence entries.
            if channel.options.presence && !entries.is_empty() {
                // Extract actor_id from the first presence entry.
                let actor_id = entries
                    .first()
                    .and_then(|e| {
                        serde_json::from_str::<serde_json::Value>(&e.member_value)
                            .ok()
                            .and_then(|v| {
                                v.get("actor_id").and_then(|a| a.as_str().map(String::from))
                            })
                    })
                    .unwrap_or_else(|| format!("anon:{connection_id}"));
                let leave_msg = ServerMessage {
                    channel: message.channel.clone(),
                    event: PRESENCE_LEAVE_EVENT,
                    room: message.room.clone(),
                    payload: serde_json::json!({
                        "actor_id": actor_id,
                    }),
                };
                state.hub.broadcast(&leave_msg).await;
            }

            state
                .hub
                .unsubscribe(connection_id, &message.channel, message.room.clone())
                .await;
            state
                .hub
                .send(
                    connection_id,
                    WriterCommand::Json(ServerMessage {
                        channel: message.channel,
                        event: UNSUBSCRIBED_EVENT,
                        room: message.room,
                        payload: serde_json::Value::Null,
                    }),
                )
                .await?;
        }
        ClientAction::Message => {
            let actor = match state.authorize_channel(connection_id, channel).await {
                Ok(actor) => actor,
                Err(error) => {
                    state
                        .hub
                        .send(
                            connection_id,
                            WriterCommand::Json(ServerMessage {
                                channel: SYSTEM_CHANNEL,
                                event: ERROR_EVENT,
                                room: None,
                                payload: error.payload(),
                            }),
                        )
                        .await
                        .ok();
                    return Ok(());
                }
            };
            let context = WebSocketContext::new(
                state.app.clone(),
                connection_id,
                actor,
                message.channel,
                message.room,
            );
            let result = channel
                .handler
                .handle(context, message.payload.unwrap_or(serde_json::Value::Null))
                .await;

            // Send ACK if requested.
            if let Some(ack_id) = message.ack_id {
                let (status, error) = match &result {
                    Ok(()) => ("ok", None),
                    Err(e) => ("error", Some(e.to_string())),
                };
                let _ = state
                    .hub
                    .send(
                        connection_id,
                        WriterCommand::Json(ServerMessage {
                            channel: SYSTEM_CHANNEL,
                            event: ACK_EVENT,
                            room: None,
                            payload: serde_json::json!({
                                "ack_id": ack_id,
                                "status": status,
                                "error": error,
                            }),
                        }),
                    )
                    .await;
            }

            result?;
        }
        ClientAction::ClientEvent => {
            if !channel.options.allow_client_events {
                return Err(Error::message("client events not allowed on this channel"));
            }

            if let Err(error) = state.authorize_channel(connection_id, channel).await {
                state
                    .hub
                    .send(
                        connection_id,
                        WriterCommand::Json(ServerMessage {
                            channel: SYSTEM_CHANNEL,
                            event: ERROR_EVENT,
                            room: None,
                            payload: error.payload(),
                        }),
                    )
                    .await
                    .ok();
                return Ok(());
            }

            let event_id = message
                .event
                .unwrap_or_else(|| ChannelEventId::new("client_event"));
            let server_msg = ServerMessage {
                channel: message.channel,
                event: event_id,
                room: message.room,
                payload: message.payload.unwrap_or(serde_json::Value::Null),
            };

            // Broadcast to all subscribers EXCEPT the sender.
            state.hub.broadcast_except(connection_id, &server_msg).await;
        }
    }

    Ok(())
}

#[derive(Clone)]
struct ConnectionHub {
    next_id: Arc<AtomicU64>,
    connections: Arc<RwLock<HashMap<u64, ConnectionState>>>,
    user_connections: Arc<RwLock<HashMap<String, HashSet<u64>>>>,
    diagnostics: Option<Arc<RuntimeDiagnostics>>,
}

impl ConnectionHub {
    fn new(diagnostics: Option<Arc<RuntimeDiagnostics>>) -> Self {
        Self {
            next_id: Arc::new(AtomicU64::new(0)),
            connections: Arc::new(RwLock::new(HashMap::new())),
            user_connections: Arc::new(RwLock::new(HashMap::new())),
            diagnostics,
        }
    }
}

impl ConnectionHub {
    async fn register(
        &self,
        identity: ConnectionIdentity,
    ) -> (
        u64,
        mpsc::UnboundedReceiver<WriterCommand>,
        Arc<tokio::sync::Mutex<tokio::time::Instant>>,
    ) {
        let connection_id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::unbounded_channel();
        let last_pong_at = Arc::new(tokio::sync::Mutex::new(tokio::time::Instant::now()));
        let client_ip = identity.client_ip.clone();
        self.connections.write().await.insert(
            connection_id,
            ConnectionState {
                subscriptions: HashSet::new(),
                presence_entries: Vec::new(),
                identity,
                actors: HashMap::new(),
                sender: tx,
                message_count: 0,
                rate_window_start: tokio::time::Instant::now(),
            },
        );
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.record_websocket_connection(WebSocketConnectionState::Opened);
        }
        // Track anonymous connections by IP
        if let Some(ref ip) = client_ip {
            let tracking_key = format!("ip:{ip}");
            self.user_connections
                .write()
                .await
                .entry(tracking_key)
                .or_default()
                .insert(connection_id);
        }

        tracing::info!(
            target: "forge.websocket",
            connection_id = connection_id,
            "WebSocket connection opened"
        );
        (connection_id, rx, last_pong_at)
    }

    async fn unregister(&self, connection_id: u64) -> Vec<PresenceEntry> {
        let state = self.connections.write().await.remove(&connection_id);
        if let Some(state) = state {
            if let Some(diagnostics) = &self.diagnostics {
                for key in &state.subscriptions {
                    diagnostics.record_websocket_subscription_closed_on(&key.channel);
                }
                diagnostics.record_websocket_connection(WebSocketConnectionState::Closed);
            }

            // Clean up user_connections tracking (single lock acquisition).
            {
                let mut user_conns = self.user_connections.write().await;
                // Clean up actor-based tracking
                for actor in state.actors.values() {
                    if let Some(set) = user_conns.get_mut(&actor.id) {
                        set.remove(&connection_id);
                        if set.is_empty() {
                            user_conns.remove(&actor.id);
                        }
                    }
                }
                // Clean up IP-based tracking (for anonymous connections)
                if let Some(ref ip) = state.identity.client_ip {
                    let ip_key = format!("ip:{ip}");
                    if let Some(set) = user_conns.get_mut(&ip_key) {
                        set.remove(&connection_id);
                        if set.is_empty() {
                            user_conns.remove(&ip_key);
                        }
                    }
                }
            }

            tracing::info!(
                target: "forge.websocket",
                connection_id = connection_id,
                "WebSocket connection closed"
            );
            return state.presence_entries;
        }
        Vec::new()
    }

    async fn subscribe(
        &self,
        connection_id: u64,
        channel: &ChannelId,
        room: Option<String>,
    ) -> bool {
        if let Some(state) = self.connections.write().await.get_mut(&connection_id) {
            let created = state.subscriptions.insert(SubscriptionKey {
                channel: channel.clone(),
                room,
            });
            if created {
                if let Some(diagnostics) = &self.diagnostics {
                    diagnostics.record_websocket_subscription_opened_on(channel);
                }
            }
            return created;
        }

        false
    }

    async fn unsubscribe(
        &self,
        connection_id: u64,
        channel: &ChannelId,
        room: Option<String>,
    ) -> bool {
        if let Some(state) = self.connections.write().await.get_mut(&connection_id) {
            let removed = state.subscriptions.remove(&SubscriptionKey {
                channel: channel.clone(),
                room,
            });
            if removed {
                if let Some(diagnostics) = &self.diagnostics {
                    diagnostics.record_websocket_subscription_closed_on(channel);
                }
            }
            return removed;
        }

        false
    }

    async fn send(&self, connection_id: u64, command: WriterCommand) -> Result<()> {
        let channel = if let WriterCommand::Json(ref msg) = command {
            Some(msg.channel.clone())
        } else {
            None
        };
        let sender = self
            .connections
            .read()
            .await
            .get(&connection_id)
            .map(|state| state.sender.clone())
            .ok_or_else(|| Error::message("websocket connection not found"))?;
        sender
            .send(command)
            .map_err(|_| Error::message("websocket connection closed"))?;
        if let Some(diagnostics) = &self.diagnostics {
            if let Some(ref ch) = channel {
                diagnostics.record_websocket_outbound_message_on(ch);
            } else {
                unreachable!("WriterCommand::Ping and WriterCommand::Close are not routed through ConnectionHub::send")
            }
        }
        Ok(())
    }

    async fn broadcast(&self, message: &ServerMessage) {
        let senders = {
            let connections = self.connections.read().await;
            connections
                .values()
                .filter(|state| state.accepts(message))
                .map(|state| state.sender.clone())
                .collect::<Vec<_>>()
        };

        for sender in senders {
            let _ = sender.send(WriterCommand::Json(message.clone()));
        }
    }

    async fn broadcast_except(&self, exclude_id: u64, message: &ServerMessage) {
        let senders = {
            let connections = self.connections.read().await;
            connections
                .iter()
                .filter(|(id, state)| **id != exclude_id && state.accepts(message))
                .map(|(_, state)| state.sender.clone())
                .collect::<Vec<_>>()
        };

        for sender in senders {
            let _ = sender.send(WriterCommand::Json(message.clone()));
        }
    }

    /// Returns a clone of the sender for the given connection, used by the heartbeat task.
    async fn sender(&self, connection_id: u64) -> Option<mpsc::UnboundedSender<WriterCommand>> {
        self.connections
            .read()
            .await
            .get(&connection_id)
            .map(|state| state.sender.clone())
    }

    async fn identity(
        &self,
        connection_id: u64,
    ) -> std::result::Result<ConnectionIdentity, AuthError> {
        self.connections
            .read()
            .await
            .get(&connection_id)
            .map(|state| state.identity.clone())
            .ok_or_else(|| AuthError::internal("websocket connection not found"))
    }

    async fn cached_actor(
        &self,
        connection_id: u64,
        guard: &GuardId,
    ) -> std::result::Result<Option<Actor>, AuthError> {
        self.connections
            .read()
            .await
            .get(&connection_id)
            .map(|state| state.actors.get(guard).cloned())
            .ok_or_else(|| AuthError::internal("websocket connection not found"))
    }

    async fn cache_actor(
        &self,
        connection_id: u64,
        actor: Actor,
        max_connections_per_user: u32,
    ) -> std::result::Result<(), AuthError> {
        let actor_id = actor.id.clone();
        let guard = actor.guard.clone();

        // Acquire both locks — check + insert atomically to prevent TOCTOU race
        let mut user_conns = self.user_connections.write().await;

        if max_connections_per_user > 0 {
            if let Some(existing) = user_conns.get(&actor_id) {
                if existing.len() >= max_connections_per_user as usize {
                    return Err(AuthError::forbidden_code(
                        AuthErrorCode::MaxConnectionsPerUserExceeded,
                    ));
                }
            }
        }

        let mut connections = self.connections.write().await;
        let state = connections
            .get_mut(&connection_id)
            .ok_or_else(|| AuthError::internal("websocket connection not found"))?;
        state.actors.insert(guard, actor);

        // Remove IP-based tracking (anonymous → authenticated transition)
        if let Some(ref ip) = state.identity.client_ip {
            let ip_key = format!("ip:{ip}");
            if let Some(set) = user_conns.get_mut(&ip_key) {
                set.remove(&connection_id);
                if set.is_empty() {
                    user_conns.remove(&ip_key);
                }
            }
        }

        // Track by actor ID
        user_conns
            .entry(actor_id)
            .or_default()
            .insert(connection_id);

        Ok(())
    }

    async fn add_presence_entry(&self, connection_id: u64, entry: PresenceEntry) {
        if let Some(state) = self.connections.write().await.get_mut(&connection_id) {
            state.presence_entries.push(entry);
        }
    }

    async fn take_presence_entries_for_channel(
        &self,
        connection_id: u64,
        channel: &ChannelId,
    ) -> Vec<PresenceEntry> {
        let mut connections = self.connections.write().await;
        let Some(state) = connections.get_mut(&connection_id) else {
            return Vec::new();
        };
        let key_prefix = presence_key(channel);
        let (matching, remaining): (Vec<_>, Vec<_>) = state
            .presence_entries
            .drain(..)
            .partition(|e| e.key == key_prefix);
        state.presence_entries = remaining;
        matching
    }

    /// Per-connection rate limiting. Returns `true` if the message is allowed.
    async fn check_rate_limit(&self, connection_id: u64, max_per_second: u32) -> bool {
        if max_per_second == 0 {
            return true; // unlimited
        }
        let mut connections = self.connections.write().await;
        let Some(state) = connections.get_mut(&connection_id) else {
            return false;
        };
        if state.rate_window_start.elapsed() >= Duration::from_secs(1) {
            state.message_count = 0;
            state.rate_window_start = tokio::time::Instant::now();
        }
        state.message_count += 1;
        state.message_count <= max_per_second
    }

    /// Force-disconnect all connections belonging to a given actor.
    async fn disconnect_by_actor(&self, actor_id: &str) -> Vec<PresenceEntry> {
        let mut connections = self.connections.write().await;
        let to_remove: Vec<u64> = connections
            .iter()
            .filter(|(_, state)| state.actors.values().any(|a| a.id == actor_id))
            .map(|(id, _)| *id)
            .collect();

        let mut all_presence = Vec::new();
        for id in &to_remove {
            if let Some(state) = connections.remove(id) {
                if let Some(diagnostics) = &self.diagnostics {
                    for key in &state.subscriptions {
                        diagnostics.record_websocket_subscription_closed_on(&key.channel);
                    }
                    diagnostics.record_websocket_connection(WebSocketConnectionState::Closed);
                }
                all_presence.extend(state.presence_entries);
                // Dropping the sender closes the writer task which closes the socket.
            }
        }

        // Clean up user_connections tracking.
        if !to_remove.is_empty() {
            drop(connections);
            let mut user_conns = self.user_connections.write().await;
            if let Some(set) = user_conns.get_mut(actor_id) {
                for id in &to_remove {
                    set.remove(id);
                }
                if set.is_empty() {
                    user_conns.remove(actor_id);
                }
            }
        }

        all_presence
    }
}

struct ConnectionState {
    subscriptions: HashSet<SubscriptionKey>,
    presence_entries: Vec<PresenceEntry>,
    identity: ConnectionIdentity,
    actors: HashMap<GuardId, Actor>,
    sender: mpsc::UnboundedSender<WriterCommand>,
    message_count: u32,
    rate_window_start: tokio::time::Instant,
}

impl ConnectionState {
    fn accepts(&self, message: &ServerMessage) -> bool {
        self.subscriptions.iter().any(|subscription| {
            subscription.channel == message.channel
                && (subscription.room.is_none() || subscription.room == message.room)
        })
    }
}

#[derive(Debug, Clone, Default)]
struct ConnectionIdentity {
    bearer_token: Option<String>,
    session_id: Option<String>,
    auth_error: Option<AuthError>,
    client_ip: Option<String>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SubscriptionKey {
    channel: ChannelId,
    room: Option<String>,
}

/// Tracks presence values that need cleanup on disconnect.
#[derive(Debug, Clone)]
struct PresenceEntry {
    key: String,
    member_value: String,
}

fn auth_outcome_from_error(error: &AuthError) -> AuthOutcome {
    match error {
        AuthError::Unauthorized(_) => AuthOutcome::Unauthorized,
        AuthError::Forbidden(_) => AuthOutcome::Forbidden,
        AuthError::Internal(_) => AuthOutcome::Error,
    }
}
