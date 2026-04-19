# websocket

Channel-based WebSocket with presence and typed messages

[Back to index](../index.md)

## forge::websocket

```rust
pub const ACK_EVENT: ChannelEventId;
pub const ERROR_EVENT: ChannelEventId;
pub const PRESENCE_JOIN_EVENT: ChannelEventId;
pub const PRESENCE_LEAVE_EVENT: ChannelEventId;
pub const SUBSCRIBED_EVENT: ChannelEventId;
pub const SYSTEM_CHANNEL: ChannelId;
pub const UNSUBSCRIBED_EVENT: ChannelEventId;
pub type AuthorizeCallback = Arc<dyn for<'a> Fn(&'a WebSocketContext, &'a ChannelId, Option<&'a str>) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> + Send + Sync>;
pub type LifecycleCallback = Arc<dyn for<'a> Fn(&'a WebSocketContext) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> + Send + Sync>;
pub type WebSocketRouteRegistrar = Arc<dyn Fn(&mut WebSocketRegistrar) -> Result<()> + Send + Sync>;
enum ClientAction { Subscribe, Unsubscribe, Message, ClientEvent }
struct ClientMessage
struct PresenceInfo
struct ServerMessage
struct WebSocketChannelDescriptor
struct WebSocketChannelOptions
  fn new() -> Self
  fn presence(self, enabled: bool) -> Self
  fn guard<I>(self, guard: I) -> Self
  fn permission<I>(self, permission: I) -> Self
  fn permissions<I, P>(self, permissions: I) -> Self
  fn authorize<F, Fut>(self, f: F) -> Self
  fn allow_client_events(self, enabled: bool) -> Self
  fn on_join<F, Fut>(self, f: F) -> Self
  fn on_leave<F, Fut>(self, f: F) -> Self
  fn replay(self, count: u32) -> Self
struct WebSocketChannelRegistry
  fn from_registrar(registrar: WebSocketRegistrar) -> Self
  fn descriptors(&self) -> Vec<WebSocketChannelDescriptor>
  fn find(&self, id: &ChannelId) -> Option<WebSocketChannelDescriptor>
struct WebSocketContext
  fn app(&self) -> &AppContext
  fn connection_id(&self) -> u64
  fn actor(&self) -> Option<&Actor>
  async fn resolve_actor<M: Authenticatable>(&self) -> Result<Option<M>>
  fn channel(&self) -> &ChannelId
  fn room(&self) -> Option<&str>
  async fn publish<I>(&self, event: I, payload: impl Serialize) -> Result<()>
  async fn presence_members(&self) -> Result<Vec<PresenceInfo>>
  async fn presence_count(&self) -> Result<usize>
struct WebSocketPublisher
  async fn publish<C, E>( &self, channel: C, event: E, room: Option<&str>, payload: impl Serialize, ) -> Result<()>
  async fn publish_message(&self, message: ServerMessage) -> Result<()>
  async fn disconnect_user(&self, actor_id: &str) -> Result<()>
struct WebSocketRegistrar
  fn new() -> Self
  fn channel<I, H>(&mut self, id: I, handler: H) -> Result<&mut Self>
  fn channel_with_options<I, H>( &mut self, id: I, handler: H, options: WebSocketChannelOptions, ) -> Result<&mut Self>
trait ChannelHandler
  fn handle<'life0, 'async_trait>(
```

