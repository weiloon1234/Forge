use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::auth::{AccessScope, Actor, Authenticatable};
use crate::foundation::{AppContext, Error, Result};
use crate::logging::RuntimeDiagnostics;
use crate::support::runtime::RuntimeBackend;
use crate::support::{ChannelEventId, ChannelId, GuardId, PermissionId};

pub(crate) fn presence_key(channel: &ChannelId) -> String {
    format!("ws:presence:{}", channel.as_str())
}

pub(crate) fn presence_member_value(actor_id: &str, channel: &ChannelId, joined_at: i64) -> String {
    serde_json::to_string(&PresenceInfo {
        actor_id: actor_id.to_string(),
        channel: channel.clone(),
        joined_at,
    })
    .unwrap_or_default()
}

pub type WebSocketRouteRegistrar = Arc<dyn Fn(&mut WebSocketRegistrar) -> Result<()> + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceInfo {
    pub actor_id: String,
    pub channel: ChannelId,
    pub joined_at: i64,
}

pub const SYSTEM_CHANNEL: ChannelId = ChannelId::new("system");
pub const ERROR_EVENT: ChannelEventId = ChannelEventId::new("error");
pub const SUBSCRIBED_EVENT: ChannelEventId = ChannelEventId::new("subscribed");
pub const UNSUBSCRIBED_EVENT: ChannelEventId = ChannelEventId::new("unsubscribed");
pub const PRESENCE_JOIN_EVENT: ChannelEventId = ChannelEventId::new("presence:join");
pub const PRESENCE_LEAVE_EVENT: ChannelEventId = ChannelEventId::new("presence:leave");
pub const ACK_EVENT: ChannelEventId = ChannelEventId::new("ack");

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientAction {
    Subscribe,
    Unsubscribe,
    Message,
    ClientEvent,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ClientMessage {
    pub action: ClientAction,
    pub channel: ChannelId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<ChannelEventId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ServerMessage {
    pub channel: ChannelId,
    pub event: ChannelEventId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    pub payload: serde_json::Value,
}

#[derive(Clone)]
pub struct WebSocketContext {
    app: AppContext,
    connection_id: u64,
    actor: Option<Actor>,
    channel: ChannelId,
    room: Option<String>,
}

impl WebSocketContext {
    pub(crate) fn new(
        app: AppContext,
        connection_id: u64,
        actor: Option<Actor>,
        channel: ChannelId,
        room: Option<String>,
    ) -> Self {
        Self {
            app,
            connection_id,
            actor,
            channel,
            room,
        }
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }

    pub fn connection_id(&self) -> u64 {
        self.connection_id
    }

    pub fn actor(&self) -> Option<&Actor> {
        self.actor.as_ref()
    }

    /// Resolve the authenticated actor to its backing model.
    ///
    /// Returns `Ok(None)` if no actor is present on this connection.
    pub async fn resolve_actor<M: Authenticatable>(&self) -> Result<Option<M>> {
        match &self.actor {
            Some(actor) => actor.resolve::<M>(&self.app).await,
            None => Ok(None),
        }
    }

    pub fn channel(&self) -> &ChannelId {
        &self.channel
    }

    pub fn room(&self) -> Option<&str> {
        self.room.as_deref()
    }

    pub async fn publish<I>(&self, event: I, payload: impl Serialize) -> Result<()>
    where
        I: Into<ChannelEventId>,
    {
        self.app
            .websocket()?
            .publish(self.channel.clone(), event, self.room(), payload)
            .await
    }

    /// Return all presence members for the current channel.
    pub async fn presence_members(&self) -> Result<Vec<PresenceInfo>> {
        let backend = RuntimeBackend::from_config(self.app.config())?;
        let key = presence_key(&self.channel);
        let members = backend.smembers(&key).await?;
        let mut infos = Vec::with_capacity(members.len());
        for raw in members {
            if let Ok(info) = serde_json::from_str::<PresenceInfo>(&raw) {
                infos.push(info);
            }
        }
        Ok(infos)
    }

    /// Return the number of presence members for the current channel.
    pub async fn presence_count(&self) -> Result<usize> {
        let backend = RuntimeBackend::from_config(self.app.config())?;
        let key = presence_key(&self.channel);
        backend.scard(&key).await
    }
}

#[async_trait]
pub trait ChannelHandler: Send + Sync + 'static {
    async fn handle(&self, context: WebSocketContext, payload: serde_json::Value) -> Result<()>;
}

#[async_trait]
impl<F, Fut> ChannelHandler for F
where
    F: Fn(WebSocketContext, serde_json::Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<()>> + Send + 'static,
{
    async fn handle(&self, context: WebSocketContext, payload: serde_json::Value) -> Result<()> {
        (self)(context, payload).await
    }
}

#[derive(Clone)]
pub struct WebSocketPublisher {
    backend: RuntimeBackend,
    diagnostics: Arc<RuntimeDiagnostics>,
}

impl WebSocketPublisher {
    pub(crate) fn new(backend: RuntimeBackend, diagnostics: Arc<RuntimeDiagnostics>) -> Self {
        Self {
            backend,
            diagnostics,
        }
    }

    pub async fn publish<C, E>(
        &self,
        channel: C,
        event: E,
        room: Option<&str>,
        payload: impl Serialize,
    ) -> Result<()>
    where
        C: Into<ChannelId>,
        E: Into<ChannelEventId>,
    {
        self.publish_message(ServerMessage {
            channel: channel.into(),
            event: event.into(),
            room: room.map(ToOwned::to_owned),
            payload: serde_json::to_value(payload).map_err(Error::other)?,
        })
        .await
    }

    pub async fn publish_message(&self, message: ServerMessage) -> Result<()> {
        let payload = serde_json::to_string(&message).map_err(Error::other)?;
        self.diagnostics.record_websocket_outbound_message();
        self.backend
            .publish_ws(message.channel.as_str(), &payload)
            .await?;

        // Buffer for replay so new subscribers can catch up on recent messages.
        let history_key = format!("ws:history:{}", message.channel);
        let _ = self.backend.lpush_capped(&history_key, &payload, 50).await;

        Ok(())
    }

    /// Force disconnect all connections for a specific user (across all instances).
    pub async fn disconnect_user(&self, actor_id: &str) -> Result<()> {
        let command = serde_json::json!({
            "type": "disconnect_user",
            "actor_id": actor_id,
        });
        self.backend
            .publish_ws("__system:disconnect", &command.to_string())
            .await
    }
}

pub struct WebSocketRegistrar {
    channels: HashMap<ChannelId, RegisteredChannel>,
}

/// Type for channel lifecycle callbacks (on_join / on_leave).
pub type LifecycleCallback = Arc<
    dyn for<'a> Fn(
            &'a WebSocketContext,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>
        + Send
        + Sync,
>;

/// Type for dynamic per-subscription authorization callbacks.
pub type AuthorizeCallback = Arc<
    dyn for<'a> Fn(
            &'a WebSocketContext,
            &'a ChannelId,
            Option<&'a str>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>
        + Send
        + Sync,
>;

#[derive(Clone, Default)]
pub struct WebSocketChannelOptions {
    pub access: AccessScope,
    pub presence: bool,
    pub(crate) authorize: Option<AuthorizeCallback>,
    pub(crate) allow_client_events: bool,
    pub(crate) on_join: Option<LifecycleCallback>,
    pub(crate) on_leave: Option<LifecycleCallback>,
    pub(crate) replay_count: u32,
}

impl WebSocketChannelOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn presence(mut self, enabled: bool) -> Self {
        self.presence = enabled;
        self
    }

    pub fn guard<I>(mut self, guard: I) -> Self
    where
        I: Into<GuardId>,
    {
        self.access = self.access.with_guard(guard);
        self
    }

    pub fn permission<I>(mut self, permission: I) -> Self
    where
        I: Into<PermissionId>,
    {
        self.access = self.access.with_permission(permission);
        self
    }

    pub fn permissions<I, P>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PermissionId>,
    {
        self.access = self.access.with_permissions(permissions);
        self
    }

    /// Add a dynamic authorization callback for subscription requests.
    ///
    /// Called after guard/permission checks. Return `Ok(())` to allow,
    /// `Err(...)` to reject.
    ///
    /// ```ignore
    /// WebSocketChannelOptions::new()
    ///     .guard(AuthGuard::Api)
    ///     .authorize(|ctx, channel, room| async move {
    ///         let actor = ctx.actor().ok_or(Error::unauthorized("auth required"))?;
    ///         // Custom logic...
    ///         Ok(())
    ///     })
    /// ```
    pub fn authorize<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(&WebSocketContext, &ChannelId, Option<&str>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.authorize = Some(Arc::new(move |ctx, ch, room| Box::pin(f(ctx, ch, room))));
        self
    }

    /// Allow clients to send events that are relayed to other subscribers.
    pub fn allow_client_events(mut self, enabled: bool) -> Self {
        self.allow_client_events = enabled;
        self
    }

    /// Register a callback invoked when a client subscribes to this channel.
    pub fn on_join<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(&WebSocketContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.on_join = Some(Arc::new(move |ctx| Box::pin(f(ctx))));
        self
    }

    /// Register a callback invoked when a client unsubscribes from this channel.
    pub fn on_leave<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(&WebSocketContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.on_leave = Some(Arc::new(move |ctx| Box::pin(f(ctx))));
        self
    }

    /// Enable message replay for new subscribers on this channel.
    ///
    /// When a client subscribes, the last `count` messages are sent before
    /// the `SUBSCRIBED` event so the client can catch up on recent activity.
    /// Set to `0` (the default) to disable replay.
    pub fn replay(mut self, count: u32) -> Self {
        self.replay_count = count;
        self
    }

    pub(crate) fn requires_auth(&self) -> bool {
        self.access.requires_auth()
    }

    pub(crate) fn guard_id(&self) -> Option<&GuardId> {
        self.access.guard()
    }

    pub(crate) fn permissions_set(&self) -> std::collections::BTreeSet<PermissionId> {
        self.access.permissions()
    }
}

impl Default for WebSocketRegistrar {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSocketRegistrar {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }

    pub fn channel<I, H>(&mut self, id: I, handler: H) -> Result<&mut Self>
    where
        I: Into<ChannelId>,
        H: ChannelHandler,
    {
        self.channel_with_options(id, handler, WebSocketChannelOptions::default())
    }

    pub fn channel_with_options<I, H>(
        &mut self,
        id: I,
        handler: H,
        options: WebSocketChannelOptions,
    ) -> Result<&mut Self>
    where
        I: Into<ChannelId>,
        H: ChannelHandler,
    {
        let id = id.into();
        if self.channels.contains_key(&id) {
            return Err(Error::message(format!(
                "websocket channel `{id}` already registered"
            )));
        }

        self.channels.insert(
            id.clone(),
            RegisteredChannel {
                id,
                options,
                handler: Arc::new(handler),
            },
        );
        Ok(self)
    }

    pub(crate) fn into_channels(self) -> Vec<RegisteredChannel> {
        self.channels.into_values().collect()
    }
}

#[derive(Clone)]
pub(crate) struct RegisteredChannel {
    pub id: ChannelId,
    pub options: WebSocketChannelOptions,
    pub handler: Arc<dyn ChannelHandler>,
}

#[cfg(test)]
mod tests {
    use super::{ChannelId, WebSocketRegistrar};

    #[test]
    fn rejects_duplicate_channel_registration() {
        let mut registrar = WebSocketRegistrar::new();
        registrar
            .channel(ChannelId::new("chat"), |_context, _payload| async {
                Ok(())
            })
            .unwrap();

        let error = registrar
            .channel(ChannelId::new("chat"), |_context, _payload| async {
                Ok(())
            })
            .err()
            .unwrap();
        assert!(error.to_string().contains("already registered"));
    }
}
