use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use forge::prelude::*;
use tempfile::tempdir;

mod app {
    use super::*;

    pub mod ids {
        use super::*;

        pub const MOBILE_RULE: ValidationRuleId = ValidationRuleId::new("mobile");
        pub const HELLO_COMMAND: CommandId = CommandId::new("hello");
        pub const HEARTBEAT_SCHEDULE: ScheduleId = ScheduleId::new("heartbeat");
    }

    pub mod providers {
        use super::*;

        #[derive(Clone)]
        pub struct AppServiceProvider {
            pub events: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait]
        impl ServiceProvider for AppServiceProvider {
            async fn register(&self, registrar: &mut ServiceRegistrar) -> Result<()> {
                registrar.singleton_arc(self.events.clone())?;
                Ok(())
            }

            async fn boot(&self, app: &AppContext) -> Result<()> {
                let events = app.resolve::<Mutex<Vec<String>>>()?;
                events.lock().unwrap().push("provider:boot".to_string());
                Ok(())
            }
        }
    }

    pub mod validation {
        use super::*;

        pub struct MobileRule;

        #[async_trait]
        impl ValidationRule for MobileRule {
            async fn validate(
                &self,
                _context: &RuleContext,
                value: &str,
            ) -> std::result::Result<(), ValidationError> {
                if value.starts_with('+') && value[1..].chars().all(|ch| ch.is_ascii_digit()) {
                    Ok(())
                } else {
                    Err(ValidationError::new("mobile", "invalid mobile number"))
                }
            }
        }
    }

    pub mod portals {
        use super::*;

        #[derive(Debug, Deserialize)]
        pub struct CreateUser {
            pub email: String,
            pub phone: String,
        }

        #[async_trait]
        impl RequestValidator for CreateUser {
            async fn validate(&self, validator: &mut Validator) -> Result<()> {
                validator
                    .field("email", self.email.clone())
                    .required()
                    .email()
                    .apply()
                    .await?;
                validator
                    .field("phone", self.phone.clone())
                    .required()
                    .rule(ids::MOBILE_RULE)
                    .apply()
                    .await
            }
        }

        #[async_trait]
        impl forge::validation::FromMultipart for CreateUser {
            async fn from_multipart(
                multipart: &mut axum::extract::Multipart,
            ) -> forge::foundation::Result<Self> {
                let mut email = None;
                let mut phone = None;
                while let Some(field) = multipart.next_field().await
                    .map_err(|e| forge::foundation::Error::message(format!("multipart error: {e}")))?
                {
                    match field.name().unwrap_or("") {
                        "email" => email = Some(field.text().await
                            .map_err(|e| forge::foundation::Error::message(format!("field error: {e}")))?),
                        "phone" => phone = Some(field.text().await
                            .map_err(|e| forge::foundation::Error::message(format!("field error: {e}")))?),
                        _ => {}
                    }
                }
                Ok(Self {
                    email: email.unwrap_or_default(),
                    phone: phone.unwrap_or_default(),
                })
            }
        }

        pub fn router(registrar: &mut HttpRegistrar) -> Result<()> {
            registrar.route("/health", get(health));
            registrar.route("/users", post(create_user));
            Ok(())
        }

        async fn health(State(app): State<AppContext>) -> impl IntoResponse {
            let events = app.resolve::<Mutex<Vec<String>>>().unwrap();

            Json(serde_json::json!({
                "status": "ok",
                "events": events.lock().unwrap().clone(),
            }))
        }

        async fn create_user(Validated(payload): Validated<CreateUser>) -> impl IntoResponse {
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "email": payload.email,
                    "phone": payload.phone,
                })),
            )
        }
    }

    pub mod commands {
        use super::*;

        pub fn register(registry: &mut CommandRegistry) -> Result<()> {
            registry.command(
                ids::HELLO_COMMAND,
                Command::new("hello").about("test command"),
                |invocation: CommandInvocation| async move {
                    let events = invocation.app().resolve::<Mutex<Vec<String>>>()?;
                    events.lock().unwrap().push("command:hello".to_string());
                    Ok(())
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
                |invocation| async move {
                    let events = invocation.app().resolve::<Mutex<Vec<String>>>()?;
                    events
                        .lock()
                        .unwrap()
                        .push("schedule:heartbeat".to_string());
                    Ok(())
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

fn write_config(dir: &Path, port: u16) {
    fs::write(
        dir.join("00-server.toml"),
        format!(
            r#"
            [server]
            host = "127.0.0.1"
            port = {port}
        "#
        ),
    )
    .unwrap();
}

fn build_app(config_dir: &Path, events: Arc<Mutex<Vec<String>>>) -> AppBuilder {
    App::builder()
        .load_config_dir(config_dir)
        .register_provider(app::providers::AppServiceProvider { events })
        .register_routes(app::portals::router)
        .register_commands(app::commands::register)
        .register_schedule(app::schedules::register)
        .register_validation_rule(app::ids::MOBILE_RULE, app::validation::MobileRule)
}

#[tokio::test]
async fn run_http_async_serves_routes_and_validation() {
    let config_dir = tempdir().unwrap();
    let port = free_port();
    write_config(config_dir.path(), port);

    let events = Arc::new(Mutex::new(Vec::new()));
    let task = tokio::spawn({
        let builder = build_app(config_dir.path(), events.clone());
        async move { builder.run_http_async().await.unwrap() }
    });

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}");

    for _ in 0..30 {
        if client.get(format!("{url}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let health = client.get(format!("{url}/health")).send().await.unwrap();
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    let payload: serde_json::Value = health.json().await.unwrap();
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["events"][0], "provider:boot");

    let invalid = client
        .post(format!("{url}/users"))
        .json(&serde_json::json!({
            "email": "not-an-email",
            "phone": "1234",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);

    let valid = client
        .post(format!("{url}/users"))
        .json(&serde_json::json!({
            "email": "forge@example.com",
            "phone": "+60123456789",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(valid.status(), reqwest::StatusCode::CREATED);

    task.abort();
}

#[tokio::test]
async fn cli_kernel_dispatches_registered_command() {
    let config_dir = tempdir().unwrap();
    write_config(config_dir.path(), free_port());

    let events = Arc::new(Mutex::new(Vec::new()));
    build_app(config_dir.path(), events.clone())
        .build_cli_kernel()
        .await
        .unwrap()
        .run_with_args(["forge", "hello"])
        .await
        .unwrap();

    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|entry| entry == "command:hello"));
}

#[tokio::test]
async fn scheduler_kernel_runs_registered_cron_jobs() {
    let config_dir = tempdir().unwrap();
    write_config(config_dir.path(), free_port());

    let events = Arc::new(Mutex::new(Vec::new()));
    let scheduler = build_app(config_dir.path(), events.clone())
        .build_scheduler_kernel()
        .await
        .unwrap();
    let now = DateTime::parse("2026-04-08T12:00:00Z").unwrap();

    let executed = scheduler.tick_at(now).await.unwrap();
    assert_eq!(executed, vec![app::ids::HEARTBEAT_SCHEDULE]);
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|entry| entry == "schedule:heartbeat"));
}
