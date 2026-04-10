use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::auth::{AccessScope, Actor};
use crate::foundation::{AppContext, Error, Result};
use crate::logging::RuntimeDiagnostics;
use crate::support::runtime::RuntimeBackend;
use crate::support::{ChannelEventId, ChannelId, GuardId, PermissionId};

pub type WebSocketRouteRegistrar = Arc<dyn Fn(&mut WebSocketRegistrar) -> Result<()> + Send + Sync>;

pub const SYSTEM_CHANNEL: ChannelId = ChannelId::new("system");
pub const ERROR_EVENT: ChannelEventId = ChannelEventId::new("error");
pub const SUBSCRIBED_EVENT: ChannelEventId = ChannelEventId::new("subscribed");
pub const UNSUBSCRIBED_EVENT: ChannelEventId = ChannelEventId::new("unsubscribed");

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientAction {
    Subscribe,
    Unsubscribe,
    Message,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ClientMessage {
    pub action: ClientAction,
    pub channel: ChannelId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
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
            .await
    }
}

pub struct WebSocketRegistrar {
    channels: HashMap<ChannelId, RegisteredChannel>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebSocketChannelOptions {
    pub access: AccessScope,
}

impl WebSocketChannelOptions {
    pub fn new() -> Self {
        Self::default()
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
