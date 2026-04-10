use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use forge::prelude::*;
use futures_util::{SinkExt, StreamExt};
use tempfile::tempdir;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

mod app {
    use super::*;

    pub mod ids {
        use super::*;

        pub const USER_CREATED: EventId = EventId::new("user.created");
        pub const AUDIT_JOB: JobId = JobId::new("audit.job");
        pub const CHAT_CHANNEL: ChannelId = ChannelId::new("chat");
        pub const ECHO_EVENT: ChannelEventId = ChannelEventId::new("echo");
        pub const HTTP_NOTICE_EVENT: ChannelEventId = ChannelEventId::new("http_notice");
    }

    pub mod domain {
        use super::*;

        #[derive(Clone, Serialize)]
        pub struct UserCreated {
            pub email: String,
        }

        impl Event for UserCreated {
            const ID: EventId = ids::USER_CREATED;
        }

        #[derive(Debug, Serialize, Deserialize)]
        pub struct AuditJob {
            pub marker: String,
        }

        #[async_trait]
        impl Job for AuditJob {
            const ID: JobId = ids::AUDIT_JOB;

            async fn handle(&self, context: JobContext) -> Result<()> {
                let log = context.app().resolve::<Mutex<Vec<String>>>()?;
                log.lock().unwrap().push(format!("job:{}", self.marker));
                Ok(())
            }

            fn backoff(&self, _attempt: u32) -> Duration {
                Duration::from_millis(10)
            }
        }
    }

    pub mod providers {
        use super::*;

        #[derive(Clone)]
        pub struct AppServiceProvider {
            pub log: Arc<Mutex<Vec<String>>>,
            pub spawn_worker: bool,
        }

        #[async_trait]
        impl ServiceProvider for AppServiceProvider {
            async fn register(&self, registrar: &mut ServiceRegistrar) -> Result<()> {
                registrar.singleton_arc(self.log.clone())?;
                registrar.listen_event::<domain::UserCreated, _>(dispatch_job(
                    |event: &domain::UserCreated| domain::AuditJob {
                        marker: format!("event:{}", event.email),
                    },
                ))?;
                registrar.register_job::<domain::AuditJob>()?;
                Ok(())
            }

            async fn boot(&self, app: &AppContext) -> Result<()> {
                self.log.lock().unwrap().push("provider:boot".to_string());
                if self.spawn_worker {
                    spawn_worker(app.clone())?;
                }
                Ok(())
            }
        }
    }

    pub mod http {
        use super::*;

        pub fn router(registrar: &mut HttpRegistrar) -> Result<()> {
            registrar.route("/dispatch", post(dispatch_job_and_publish));
            registrar.route("/events", post(dispatch_event));
            registrar.route("/health", get(health));
            Ok(())
        }

        async fn dispatch_job_and_publish(State(app): State<AppContext>) -> impl IntoResponse {
            app.jobs()
                .unwrap()
                .dispatch(domain::AuditJob {
                    marker: "http".to_string(),
                })
                .await
                .unwrap();
            app.websocket()
                .unwrap()
                .publish(
                    ids::CHAT_CHANNEL,
                    ids::HTTP_NOTICE_EVENT,
                    None,
                    serde_json::json!({ "source": "http" }),
                )
                .await
                .unwrap();
            StatusCode::ACCEPTED
        }

        async fn dispatch_event(State(app): State<AppContext>) -> impl IntoResponse {
            app.events()
                .unwrap()
                .dispatch(domain::UserCreated {
                    email: "forge@example.com".to_string(),
                })
                .await
                .unwrap();
            StatusCode::ACCEPTED
        }

        async fn health(State(app): State<AppContext>) -> impl IntoResponse {
            let log = app.resolve::<Mutex<Vec<String>>>().unwrap();
            Json(serde_json::json!({
                "entries": log.lock().unwrap().clone(),
            }))
        }
    }

    pub mod realtime {
        use super::*;

        pub fn register(registrar: &mut WebSocketRegistrar) -> Result<()> {
            registrar.channel(
                ids::CHAT_CHANNEL,
                |context: WebSocketContext, payload: serde_json::Value| async move {
                    context.publish(ids::ECHO_EVENT, payload).await
                },
            )?;
            Ok(())
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn write_phase2_config(dir: &Path, server_port: u16, websocket_port: u16, namespace: &str) {
    fs::write(
        dir.join("00-runtime.toml"),
        format!(
            r#"
            [server]
            host = "127.0.0.1"
            port = {server_port}

            [websocket]
            host = "127.0.0.1"
            port = {websocket_port}
            path = "/ws"

            [redis]
            namespace = "{namespace}"

            [jobs]
            queue = "default"
            max_retries = 3
            poll_interval_ms = 20
        "#
        ),
    )
    .unwrap();
}

fn build_http_app(
    config_dir: &Path,
    log: Arc<Mutex<Vec<String>>>,
    spawn_worker: bool,
) -> AppBuilder {
    App::builder()
        .load_config_dir(config_dir)
        .register_provider(app::providers::AppServiceProvider { log, spawn_worker })
        .register_routes(app::http::router)
}

fn build_websocket_app(
    config_dir: &Path,
    log: Arc<Mutex<Vec<String>>>,
    spawn_worker: bool,
) -> AppBuilder {
    App::builder()
        .load_config_dir(config_dir)
        .register_provider(app::providers::AppServiceProvider { log, spawn_worker })
        .register_websocket_routes(app::realtime::register)
}

async fn wait_for_http_ready(base_url: &str) {
    let client = reqwest::Client::new();
    for _ in 0..40 {
        if client
            .get(format!("{base_url}/health"))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("http server did not become ready");
}

async fn wait_for_log(log: &Arc<Mutex<Vec<String>>>, expected: &str) {
    for _ in 0..40 {
        if log.lock().unwrap().iter().any(|entry| entry == expected) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("log entry `{expected}` not observed");
}

async fn connect_websocket(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    for _ in 0..40 {
        if let Ok((socket, _)) = connect_async(url).await {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("websocket server did not become ready");
}

#[tokio::test]
async fn websocket_kernel_handles_subscribe_message_and_unsubscribe() {
    let config_dir = tempdir().unwrap();
    let server_port = free_port();
    let websocket_port = free_port();
    write_phase2_config(
        config_dir.path(),
        server_port,
        websocket_port,
        &format!("phase2-ws-{websocket_port}"),
    );

    let log = Arc::new(Mutex::new(Vec::new()));
    let server = tokio::spawn({
        let builder = build_websocket_app(config_dir.path(), log.clone(), false);
        async move { builder.run_websocket_async().await.unwrap() }
    });

    let mut socket = connect_websocket(&format!("ws://127.0.0.1:{websocket_port}/ws")).await;
    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage {
                action: ClientAction::Subscribe,
                channel: app::ids::CHAT_CHANNEL,
                room: None,
                payload: None,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let subscribed = socket.next().await.unwrap().unwrap();
    let subscribed: ServerMessage = serde_json::from_str(subscribed.to_text().unwrap()).unwrap();
    assert_eq!(subscribed.event, SUBSCRIBED_EVENT);

    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage {
                action: ClientAction::Message,
                channel: app::ids::CHAT_CHANNEL,
                room: None,
                payload: Some(serde_json::json!({ "body": "hello" })),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let echoed = socket.next().await.unwrap().unwrap();
    let echoed: ServerMessage = serde_json::from_str(echoed.to_text().unwrap()).unwrap();
    assert_eq!(echoed.event, app::ids::ECHO_EVENT);
    assert_eq!(echoed.payload["body"], "hello");

    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage {
                action: ClientAction::Unsubscribe,
                channel: app::ids::CHAT_CHANNEL,
                room: None,
                payload: None,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let unsubscribed = socket.next().await.unwrap().unwrap();
    let unsubscribed: ServerMessage =
        serde_json::from_str(unsubscribed.to_text().unwrap()).unwrap();
    assert_eq!(unsubscribed.event, UNSUBSCRIBED_EVENT);

    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage {
                action: ClientAction::Message,
                channel: app::ids::CHAT_CHANNEL,
                room: None,
                payload: Some(serde_json::json!({ "body": "ignored" })),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let next = tokio::time::timeout(Duration::from_millis(200), socket.next()).await;
    assert!(next.is_err());

    server.abort();
}

#[tokio::test]
async fn http_handler_dispatches_job_and_publishes_to_websocket() {
    let config_dir = tempdir().unwrap();
    let server_port = free_port();
    let websocket_port = free_port();
    write_phase2_config(
        config_dir.path(),
        server_port,
        websocket_port,
        &format!("phase2-http-ws-{server_port}-{websocket_port}"),
    );

    let log = Arc::new(Mutex::new(Vec::new()));
    let http_server = tokio::spawn({
        let builder = build_http_app(config_dir.path(), log.clone(), true);
        async move { builder.run_http_async().await.unwrap() }
    });
    let websocket_server = tokio::spawn({
        let builder = build_websocket_app(config_dir.path(), log.clone(), false);
        async move { builder.run_websocket_async().await.unwrap() }
    });

    let base_url = format!("http://127.0.0.1:{server_port}");
    wait_for_http_ready(&base_url).await;

    let mut socket = connect_websocket(&format!("ws://127.0.0.1:{websocket_port}/ws")).await;
    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage {
                action: ClientAction::Subscribe,
                channel: app::ids::CHAT_CHANNEL,
                room: None,
                payload: None,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let _ = socket.next().await.unwrap().unwrap();

    reqwest::Client::new()
        .post(format!("{base_url}/dispatch"))
        .send()
        .await
        .unwrap();

    let pushed = socket.next().await.unwrap().unwrap();
    let pushed: ServerMessage = serde_json::from_str(pushed.to_text().unwrap()).unwrap();
    assert_eq!(pushed.event, app::ids::HTTP_NOTICE_EVENT);
    assert_eq!(pushed.payload["source"], "http");

    wait_for_log(&log, "job:http").await;

    http_server.abort();
    websocket_server.abort();
}

#[tokio::test]
async fn provider_registered_event_listener_dispatches_a_queued_job() {
    let config_dir = tempdir().unwrap();
    let server_port = free_port();
    let websocket_port = free_port();
    write_phase2_config(
        config_dir.path(),
        server_port,
        websocket_port,
        &format!("phase2-events-{server_port}"),
    );

    let log = Arc::new(Mutex::new(Vec::new()));
    let http_server = tokio::spawn({
        let builder = build_http_app(config_dir.path(), log.clone(), true);
        async move { builder.run_http_async().await.unwrap() }
    });

    let base_url = format!("http://127.0.0.1:{server_port}");
    wait_for_http_ready(&base_url).await;

    reqwest::Client::new()
        .post(format!("{base_url}/events"))
        .send()
        .await
        .unwrap();

    wait_for_log(&log, "job:event:forge@example.com").await;

    http_server.abort();
}
