use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, RwLock};

use crate::auth::{Actor, AuthError};
use crate::foundation::{AppContext, Error, Result};
use crate::logging::{AuthOutcome, RuntimeDiagnostics, WebSocketConnectionState};
use crate::support::runtime::RuntimeBackend;
use crate::support::{ChannelId, GuardId};
use crate::websocket::{
    ClientAction, ClientMessage, RegisteredChannel, ServerMessage, WebSocketContext,
    WebSocketRegistrar, WebSocketRouteRegistrar, ERROR_EVENT, SUBSCRIBED_EVENT, SYSTEM_CHANNEL,
    UNSUBSCRIBED_EVENT,
};

pub struct WebSocketKernel {
    app: AppContext,
    routes: Vec<WebSocketRouteRegistrar>,
}

impl WebSocketKernel {
    pub fn new(app: AppContext, routes: Vec<WebSocketRouteRegistrar>) -> Self {
        Self { app, routes }
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
        let websocket = self.app.config().websocket()?;
        let mut registrar = WebSocketRegistrar::new();
        for route in &self.routes {
            route(&mut registrar)?;
        }

        let registered_channels = registrar.into_channels();
        let state = WebSocketServerState::new(self.app.clone(), registered_channels);
        state.start_pubsub().await?;

        Ok(axum::Router::new()
            .route(&websocket.path, get(websocket_handler))
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

#[derive(Clone)]
struct WebSocketServerState {
    app: AppContext,
    channels: Arc<HashMap<ChannelId, RegisteredChannel>>,
    hub: ConnectionHub,
}

impl WebSocketServerState {
    fn new(app: AppContext, channels: Vec<RegisteredChannel>) -> Self {
        let map = channels
            .into_iter()
            .map(|channel| (channel.id.clone(), channel))
            .collect::<HashMap<_, _>>();
        let diagnostics = app.diagnostics().ok();
        Self {
            app,
            channels: Arc::new(map),
            hub: ConnectionHub::new(diagnostics),
        }
    }

    async fn start_pubsub(&self) -> Result<()> {
        if self.channels.is_empty() {
            return Ok(());
        }

        let backend = RuntimeBackend::from_config(self.app.config())?;
        let topics = self
            .channels
            .keys()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>();
        let hub = self.hub.clone();
        tokio::spawn(async move {
            let mut subscription = match backend.subscribe_ws(&topics).await {
                Ok(subscription) => subscription,
                Err(error) => {
                    tracing::error!("forge websocket pubsub startup failed: {error}");
                    return;
                }
            };

            while let Some(message) = subscription.recv().await {
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
                auth_error: Some("auth manager is not available".to_string()),
            };
        };

        if !headers.contains_key(axum::http::header::AUTHORIZATION) {
            return ConnectionIdentity::default();
        }

        match auth.extract_token(headers) {
            Ok(token) => ConnectionIdentity {
                bearer_token: Some(token),
                auth_error: None,
            },
            Err(error) => ConnectionIdentity {
                bearer_token: None,
                auth_error: Some(error.to_string()),
            },
        }
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
            let error = AuthError::unauthorized(error);
            self.record_auth_outcome(auth_outcome_from_error(&error));
            return Err(error);
        }
        let token = identity
            .bearer_token
            .ok_or_else(|| AuthError::unauthorized("missing authorization header"))
            .inspect_err(|error| self.record_auth_outcome(auth_outcome_from_error(error)))?;
        let actor = match auth.authenticate_token(&token, Some(&guard_id)).await {
            Ok(actor) => actor,
            Err(error) => {
                self.record_auth_outcome(auth_outcome_from_error(&error));
                return Err(error);
            }
        };
        let permissions = channel.options.permissions_set();
        if let Err(error) = authorizer.authorize_permissions(&actor, &permissions).await {
            self.record_auth_outcome(auth_outcome_from_error(&error));
            return Err(error);
        }
        self.hub.cache_actor(connection_id, actor.clone()).await?;
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
    headers: HeaderMap,
    State(state): State<WebSocketServerState>,
) -> Response {
    let identity = state.capture_identity(&headers).await;
    ws.on_upgrade(move |socket| handle_socket(socket, state, identity))
}

async fn handle_socket(
    socket: WebSocket,
    state: WebSocketServerState,
    identity: ConnectionIdentity,
) {
    let (connection_id, mut outbound) = state.hub.register(identity).await;
    let (mut sender, mut receiver) = socket.split();

    let writer = tokio::spawn(async move {
        while let Some(message) = outbound.recv().await {
            let payload = match serde_json::to_string(&message) {
                Ok(payload) => payload,
                Err(_) => continue,
            };
            if sender.send(Message::Text(payload.into())).await.is_err() {
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
                            ServerMessage {
                                channel: SYSTEM_CHANNEL,
                                event: ERROR_EVENT,
                                room: None,
                                payload: serde_json::json!({
                                    "message": error.to_string(),
                                }),
                            },
                        )
                        .await;
                }
            }
            Message::Close(_) => break,
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
        }
    }

    state.hub.unregister(connection_id).await;
    writer.abort();
}

async fn process_client_message(
    state: &WebSocketServerState,
    connection_id: u64,
    payload: String,
) -> Result<()> {
    if let Ok(diagnostics) = state.app.diagnostics() {
        diagnostics.record_websocket_inbound_message();
    }
    let message: ClientMessage = serde_json::from_str(&payload).map_err(Error::other)?;
    let Some(channel) = state.channels.get(&message.channel) else {
        return Err(Error::message(format!(
            "websocket channel `{}` is not registered",
            message.channel
        )));
    };

    match message.action {
        ClientAction::Subscribe => {
            let _actor = state
                .authorize_channel(connection_id, channel)
                .await
                .map_err(Error::other)?;
            state
                .hub
                .subscribe(connection_id, &message.channel, message.room.clone())
                .await;
            state
                .hub
                .send(
                    connection_id,
                    ServerMessage {
                        channel: message.channel,
                        event: SUBSCRIBED_EVENT,
                        room: message.room,
                        payload: serde_json::Value::Null,
                    },
                )
                .await?;
        }
        ClientAction::Unsubscribe => {
            state
                .hub
                .unsubscribe(connection_id, &message.channel, message.room.clone())
                .await;
            state
                .hub
                .send(
                    connection_id,
                    ServerMessage {
                        channel: message.channel,
                        event: UNSUBSCRIBED_EVENT,
                        room: message.room,
                        payload: serde_json::Value::Null,
                    },
                )
                .await?;
        }
        ClientAction::Message => {
            let actor = state
                .authorize_channel(connection_id, channel)
                .await
                .map_err(Error::other)?;
            let context = WebSocketContext::new(
                state.app.clone(),
                connection_id,
                actor,
                message.channel,
                message.room,
            );
            channel
                .handler
                .handle(context, message.payload.unwrap_or(serde_json::Value::Null))
                .await?;
        }
    }

    Ok(())
}

#[derive(Clone)]
struct ConnectionHub {
    next_id: Arc<AtomicU64>,
    connections: Arc<RwLock<HashMap<u64, ConnectionState>>>,
    diagnostics: Option<Arc<RuntimeDiagnostics>>,
}

impl ConnectionHub {
    fn new(diagnostics: Option<Arc<RuntimeDiagnostics>>) -> Self {
        Self {
            next_id: Arc::new(AtomicU64::new(0)),
            connections: Arc::new(RwLock::new(HashMap::new())),
            diagnostics,
        }
    }
}

impl ConnectionHub {
    async fn register(
        &self,
        identity: ConnectionIdentity,
    ) -> (u64, mpsc::UnboundedReceiver<ServerMessage>) {
        let connection_id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::unbounded_channel();
        self.connections.write().await.insert(
            connection_id,
            ConnectionState {
                subscriptions: HashSet::new(),
                identity,
                actors: HashMap::new(),
                sender: tx,
            },
        );
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.record_websocket_connection(WebSocketConnectionState::Opened);
        }
        (connection_id, rx)
    }

    async fn unregister(&self, connection_id: u64) {
        if let Some(state) = self.connections.write().await.remove(&connection_id) {
            if let Some(diagnostics) = &self.diagnostics {
                for _ in 0..state.subscriptions.len() {
                    diagnostics.record_websocket_subscription_closed();
                }
                diagnostics.record_websocket_connection(WebSocketConnectionState::Closed);
            }
        }
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
                    diagnostics.record_websocket_subscription_opened();
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
                    diagnostics.record_websocket_subscription_closed();
                }
            }
            return removed;
        }

        false
    }

    async fn send(&self, connection_id: u64, message: ServerMessage) -> Result<()> {
        let sender = self
            .connections
            .read()
            .await
            .get(&connection_id)
            .map(|state| state.sender.clone())
            .ok_or_else(|| Error::message("websocket connection not found"))?;
        sender
            .send(message)
            .map_err(|_| Error::message("websocket connection closed"))?;
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.record_websocket_outbound_message();
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
            let _ = sender.send(message.clone());
        }
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
    ) -> std::result::Result<(), AuthError> {
        let guard = actor.guard.clone();
        let mut connections = self.connections.write().await;
        let state = connections
            .get_mut(&connection_id)
            .ok_or_else(|| AuthError::internal("websocket connection not found"))?;
        state.actors.insert(guard, actor);
        Ok(())
    }
}

struct ConnectionState {
    subscriptions: HashSet<SubscriptionKey>,
    identity: ConnectionIdentity,
    actors: HashMap<GuardId, Actor>,
    sender: mpsc::UnboundedSender<ServerMessage>,
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
    auth_error: Option<String>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SubscriptionKey {
    channel: ChannelId,
    room: Option<String>,
}

fn auth_outcome_from_error(error: &AuthError) -> AuthOutcome {
    match error {
        AuthError::Unauthorized(_) => AuthOutcome::Unauthorized,
        AuthError::Forbidden(_) => AuthOutcome::Forbidden,
        AuthError::Internal(_) => AuthOutcome::Error,
    }
}
