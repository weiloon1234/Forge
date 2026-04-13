# events

Domain event bus with typed listeners

[Back to index](../index.md)

## forge::events

```rust
struct EventBus
  async fn dispatch<E>(&self, event: E) -> Result<()>
struct EventContext
  fn app(&self) -> &AppContext
struct JobDispatchListener
struct WebSocketPublishListener
trait Event: Serialize
trait EventListener: Event>
  fn handle<'life0, 'life1, 'life2, 'async_trait>(
fn dispatch_job<E, J, F>(mapper: F) -> JobDispatchListener<E, J, F>where E: Event, J: Job, F: Fn(&E) -> J + Send + Sync + 'static,
fn publish_websocket<E, F>(mapper: F) -> WebSocketPublishListener<E, F>where E: Event, F: Fn(&E) -> ServerMessage + Send + Sync + 'static,
```

