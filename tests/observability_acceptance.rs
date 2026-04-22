use std::fs;
use std::net::TcpListener;
use std::path::Path;
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

        #[derive(Clone, Copy)]
        pub enum ProbeKey {
            Database,
        }

        impl From<ProbeKey> for ProbeId {
            fn from(value: ProbeKey) -> Self {
                match value {
                    ProbeKey::Database => ProbeId::new("database.ready"),
                }
            }
        }

        #[derive(Clone, Copy)]
        pub enum AuthGuard {
            Api,
        }

        impl From<AuthGuard> for GuardId {
            fn from(value: AuthGuard) -> Self {
                match value {
                    AuthGuard::Api => GuardId::new("api"),
                }
            }
        }

        #[derive(Clone, Copy)]
        pub enum Ability {
            SecureView,
        }

        impl From<Ability> for PermissionId {
            fn from(value: Ability) -> Self {
                match value {
                    Ability::SecureView => PermissionId::new("secure:view"),
                }
            }
        }

        pub const AUDIT_JOB: JobId = JobId::new("audit.job");
        pub const HEARTBEAT_SCHEDULE: ScheduleId = ScheduleId::new("heartbeat");
        pub const CHAT_CHANNEL: ChannelId = ChannelId::new("chat");
        pub const ECHO_EVENT: ChannelEventId = ChannelEventId::new("echo");
    }

    pub mod domain {
        use super::*;

        #[derive(Debug, Serialize, Deserialize)]
        pub struct AuditJob {
            pub marker: String,
        }

        #[async_trait]
        impl Job for AuditJob {
            const ID: JobId = ids::AUDIT_JOB;

            async fn handle(&self, _context: JobContext) -> Result<()> {
                Ok(())
            }
        }
    }

    pub mod providers {
        use super::*;

        #[derive(Clone, Copy)]
        pub enum ProbeBehavior {
            Healthy,
            Unhealthy,
        }

        #[derive(Clone, Copy)]
        pub struct HttpServiceProvider {
            pub probe: ProbeBehavior,
        }

        pub struct DatabaseProbe {
            pub behavior: ProbeBehavior,
        }

        #[async_trait]
        impl ReadinessCheck for DatabaseProbe {
            async fn run(&self, _app: &AppContext) -> Result<ProbeResult> {
                match self.behavior {
                    ProbeBehavior::Healthy => Ok(ProbeResult::healthy(ids::ProbeKey::Database)),
                    ProbeBehavior::Unhealthy => Ok(ProbeResult::unhealthy(
                        ids::ProbeKey::Database,
                        "database offline",
                    )),
                }
            }
        }

        #[async_trait]
        impl ServiceProvider for HttpServiceProvider {
            async fn register(&self, registrar: &mut ServiceRegistrar) -> Result<()> {
                registrar.register_guard(
                    ids::AuthGuard::Api,
                    StaticBearerAuthenticator::new().token(
                        "viewer-token",
                        Actor::new("viewer-1", ids::AuthGuard::Api)
                            .with_permissions([ids::Ability::SecureView]),
                    ),
                )?;
                registrar.register_readiness_check(
                    ids::ProbeKey::Database,
                    DatabaseProbe {
                        behavior: self.probe,
                    },
                )?;
                Ok(())
            }
        }

        #[derive(Clone)]
        pub struct WorkerServiceProvider;

        #[async_trait]
        impl ServiceProvider for WorkerServiceProvider {
            async fn register(&self, registrar: &mut ServiceRegistrar) -> Result<()> {
                registrar.register_job::<domain::AuditJob>()?;
                Ok(())
            }
        }
    }

    pub mod http {
        use super::*;

        pub fn router(registrar: &mut HttpRegistrar) -> Result<()> {
            registrar.route("/public", get(public));
            registrar.route_with_options(
                "/secure",
                get(secure),
                HttpRouteOptions::new()
                    .guard(ids::AuthGuard::Api)
                    .permission(ids::Ability::SecureView),
            );
            Ok(())
        }

        async fn public(request_id: RequestId, actor: OptionalActor) -> impl IntoResponse {
            Json(serde_json::json!({
                "request_id": request_id.to_string(),
                "actor_id": actor.as_ref().map(|actor| actor.id.clone()),
            }))
        }

        async fn secure(actor: CurrentActor) -> impl IntoResponse {
            Json(serde_json::json!({
                "actor_id": actor.id,
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

    pub mod schedules {
        use super::*;

        pub fn register(registry: &mut ScheduleRegistry) -> Result<()> {
            registry.cron(
                ids::HEARTBEAT_SCHEDULE,
                CronExpression::parse("*/1 * * * * *")?,
                |_invocation| async move { Ok(()) },
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

fn write_http_config(dir: &Path, server_port: u16, namespace: &str) {
    fs::write(
        dir.join("00-runtime.toml"),
        format!(
            r#"
            [server]
            host = "127.0.0.1"
            port = {server_port}

            [redis]
            namespace = "{namespace}"
        "#
        ),
    )
    .unwrap();
}

fn write_websocket_config(dir: &Path, websocket_port: u16, namespace: &str) {
    fs::write(
        dir.join("00-runtime.toml"),
        format!(
            r#"
            [websocket]
            host = "127.0.0.1"
            port = {websocket_port}
            path = "/ws"

            [redis]
            namespace = "{namespace}"
        "#
        ),
    )
    .unwrap();
}

fn write_scheduler_config(dir: &Path, namespace: &str) {
    fs::write(
        dir.join("00-runtime.toml"),
        format!(
            r#"
            [redis]
            namespace = "{namespace}"

            [jobs]
            queue = "default"
            max_retries = 3
            poll_interval_ms = 10
        "#
        ),
    )
    .unwrap();
}

fn build_http_app(config_dir: &Path, probe: app::providers::ProbeBehavior) -> AppBuilder {
    App::builder()
        .load_config_dir(config_dir)
        .register_provider(app::providers::HttpServiceProvider { probe })
        .register_routes(app::http::router)
        .enable_observability()
}

fn build_websocket_app(config_dir: &Path) -> AppBuilder {
    App::builder()
        .load_config_dir(config_dir)
        .register_websocket_routes(app::realtime::register)
}

fn build_scheduler_app(config_dir: &Path) -> AppBuilder {
    App::builder()
        .load_config_dir(config_dir)
        .register_provider(app::providers::WorkerServiceProvider)
        .register_schedule(app::schedules::register)
}

async fn wait_for_http_ready(base_url: &str) {
    let client = reqwest::Client::new();
    for _ in 0..40 {
        if client
            .get(format!("{base_url}/_forge/health"))
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

async fn wait_for_scheduler_executions(app: &AppContext, expected: u64) {
    for _ in 0..40 {
        let snapshot = app.diagnostics().unwrap().snapshot();
        if snapshot.scheduler.executed_schedules_total >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("scheduler diagnostics did not reach the expected execution count");
}

#[tokio::test]
async fn observability_endpoints_expose_liveness_readiness_and_runtime_snapshot() {
    let config_dir = tempdir().unwrap();
    let server_port = free_port();
    write_http_config(
        config_dir.path(),
        server_port,
        &format!("observability-http-{server_port}"),
    );

    let server = tokio::spawn({
        let builder = build_http_app(config_dir.path(), app::providers::ProbeBehavior::Healthy);
        async move { builder.run_http_async().await.unwrap() }
    });

    let base_url = format!("http://127.0.0.1:{server_port}");
    wait_for_http_ready(&base_url).await;
    let client = reqwest::Client::new();

    let liveness: LivenessReport = client
        .get(format!("{base_url}/_forge/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(liveness.state, ProbeState::Healthy);

    let readiness_response = client
        .get(format!("{base_url}/_forge/ready"))
        .send()
        .await
        .unwrap();
    assert_eq!(readiness_response.status(), reqwest::StatusCode::OK);
    let readiness: ReadinessReport = readiness_response.json().await.unwrap();
    assert_eq!(readiness.state, ProbeState::Healthy);
    assert!(readiness
        .probes
        .iter()
        .any(|probe| probe.id == app::ids::ProbeKey::Database.into()));

    let public = client
        .get(format!("{base_url}/public"))
        .header("x-request-id", "observability-request")
        .send()
        .await
        .unwrap();
    assert_eq!(
        public.headers().get("x-request-id").unwrap(),
        "observability-request"
    );

    let unauthorized = client
        .get(format!("{base_url}/secure"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);

    let authorized = client
        .get(format!("{base_url}/secure"))
        .header("authorization", "Bearer viewer-token")
        .send()
        .await
        .unwrap();
    assert_eq!(authorized.status(), reqwest::StatusCode::OK);

    let snapshot: RuntimeSnapshot = client
        .get(format!("{base_url}/_forge/runtime"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot.backend, RuntimeBackendKind::Memory);
    assert!(snapshot.bootstrap_complete);
    assert!(snapshot.http.requests_total >= 5);
    assert!(snapshot.http.success_total >= 3);
    assert!(snapshot.http.client_error_total >= 1);
    assert!(snapshot.http.duration_ms.count >= 5);
    assert!(!snapshot.http.duration_ms.buckets.is_empty());
    assert!(snapshot.auth.success_total >= 1);
    assert!(snapshot.auth.unauthorized_total >= 1);

    let metrics = client
        .get(format!("{base_url}/_forge/metrics"))
        .send()
        .await
        .unwrap();
    assert_eq!(metrics.status(), reqwest::StatusCode::OK);
    let metrics_body = metrics.text().await.unwrap();
    assert!(metrics_body.contains("# TYPE forge_http_request_duration_ms histogram"));
    assert!(metrics_body.contains("forge_http_request_duration_ms_bucket{le=\"5\"}"));
    assert!(metrics_body.contains("forge_http_request_duration_ms_bucket{le=\"+Inf\"}"));
    assert!(metrics_body.contains("forge_http_request_duration_ms_sum "));
    assert!(metrics_body.contains("forge_http_request_duration_ms_count "));

    server.abort();
}

#[tokio::test]
async fn readiness_endpoint_returns_503_when_provider_probe_fails() {
    let config_dir = tempdir().unwrap();
    let server_port = free_port();
    write_http_config(
        config_dir.path(),
        server_port,
        &format!("observability-ready-{server_port}"),
    );

    let server = tokio::spawn({
        let builder = build_http_app(config_dir.path(), app::providers::ProbeBehavior::Unhealthy);
        async move { builder.run_http_async().await.unwrap() }
    });

    let base_url = format!("http://127.0.0.1:{server_port}");
    wait_for_http_ready(&base_url).await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("{base_url}/_forge/ready"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let readiness: ReadinessReport = response.json().await.unwrap();
    assert_eq!(readiness.state, ProbeState::Unhealthy);
    let database_probe = readiness
        .probes
        .into_iter()
        .find(|probe| probe.id == app::ids::ProbeKey::Database.into())
        .unwrap();
    assert_eq!(database_probe.state, ProbeState::Unhealthy);

    server.abort();
}

#[tokio::test]
async fn diagnostics_track_websocket_job_and_scheduler_activity() {
    let websocket_dir = tempdir().unwrap();
    let websocket_port = free_port();
    write_websocket_config(
        websocket_dir.path(),
        websocket_port,
        &format!("observability-ws-{websocket_port}"),
    );

    let websocket_kernel = build_websocket_app(websocket_dir.path())
        .build_websocket_kernel()
        .await
        .unwrap();
    let websocket_app = websocket_kernel.app().clone();
    let websocket_server = tokio::spawn(async move { websocket_kernel.serve().await.unwrap() });

    let mut socket = connect_websocket(&format!("ws://127.0.0.1:{websocket_port}/ws")).await;
    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage {
                action: ClientAction::Subscribe,
                channel: app::ids::CHAT_CHANNEL,
                room: None,
                payload: None,
                event: None,
                ack_id: None,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let _ = socket.next().await.unwrap().unwrap();

    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage {
                action: ClientAction::Message,
                channel: app::ids::CHAT_CHANNEL,
                room: None,
                payload: Some(serde_json::json!({ "body": "hello" })),
                event: None,
                ack_id: None,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let _ = socket.next().await.unwrap().unwrap();

    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage {
                action: ClientAction::Unsubscribe,
                channel: app::ids::CHAT_CHANNEL,
                room: None,
                payload: None,
                event: None,
                ack_id: None,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let _ = socket.next().await.unwrap().unwrap();
    socket.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let websocket_snapshot = websocket_app.diagnostics().unwrap().snapshot();
    assert!(websocket_snapshot.websocket.opened_total >= 1);
    assert!(websocket_snapshot.websocket.closed_total >= 1);
    assert!(websocket_snapshot.websocket.subscriptions_total >= 1);
    assert!(websocket_snapshot.websocket.unsubscribes_total >= 1);
    assert!(websocket_snapshot.websocket.inbound_messages_total >= 3);
    assert!(websocket_snapshot.websocket.outbound_messages_total >= 2);

    websocket_server.abort();

    let scheduler_dir = tempdir().unwrap();
    write_scheduler_config(
        scheduler_dir.path(),
        &format!("observability-jobs-{}", free_port()),
    );
    let scheduler = build_scheduler_app(scheduler_dir.path())
        .build_scheduler_kernel()
        .await
        .unwrap();
    let scheduler_app = scheduler.app().clone();

    scheduler_app
        .jobs()
        .unwrap()
        .dispatch(app::domain::AuditJob {
            marker: "manual".to_string(),
        })
        .await
        .unwrap();
    assert!(Worker::from_app(scheduler_app.clone())
        .unwrap()
        .run_once()
        .await
        .unwrap());

    let now = DateTime::parse("2026-04-08T12:00:00Z").unwrap();
    let executed = scheduler.tick_at(now).await.unwrap();
    assert_eq!(executed, vec![app::ids::HEARTBEAT_SCHEDULE]);
    wait_for_scheduler_executions(&scheduler_app, 1).await;

    let snapshot = scheduler_app.diagnostics().unwrap().snapshot();
    assert_eq!(snapshot.jobs.enqueued_total, 1);
    assert_eq!(snapshot.jobs.started_total, 1);
    assert_eq!(snapshot.jobs.succeeded_total, 1);
    assert_eq!(snapshot.jobs.retried_total, 0);
    assert_eq!(snapshot.jobs.dead_lettered_total, 0);
    assert_eq!(snapshot.scheduler.ticks_total, 1);
    assert_eq!(snapshot.scheduler.executed_schedules_total, 1);
}
