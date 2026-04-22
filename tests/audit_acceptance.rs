use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use forge::prelude::*;
use tempfile::TempDir;
use tokio::sync::{Mutex, MutexGuard};

const AUDIT_LOGS_TABLE: &str = "audit_logs";
const AUDIT_ENTRIES_TABLE: &str = "forge_test_audit_entries";
const REQUEST_ID_HEADER: &str = "x-request-id";

fn postgres_url() -> Option<String> {
    std::env::var("FORGE_TEST_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

async fn audit_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

struct AuditRuntime {
    _dir: TempDir,
    app: AppContext,
    database: Arc<DatabaseManager>,
}

impl AuditRuntime {
    async fn new() -> Option<Self> {
        let url = postgres_url()?;
        let dir = tempfile::tempdir().ok()?;
        fs::write(
            dir.path().join("00-runtime.toml"),
            format!(
                r#"
                [database]
                url = "{url}"

                [audit]
                enabled = true
                "#
            ),
        )
        .ok()?;

        let kernel = App::builder()
            .load_config_dir(dir.path())
            .build_cli_kernel()
            .await
            .ok()?;
        let app = kernel.app().clone();
        let database = app.database().ok()?;

        Some(Self {
            _dir: dir,
            app,
            database,
        })
    }

    fn config_dir(&self) -> &Path {
        self._dir.path()
    }
}

#[derive(Clone)]
struct AuditAuthProvider;

#[async_trait]
impl ServiceProvider for AuditAuthProvider {
    async fn register(&self, registrar: &mut ServiceRegistrar) -> Result<()> {
        registrar.register_guard(
            GuardId::new("admin"),
            StaticBearerAuthenticator::new()
                .token("admin-token", Actor::new("admin-1", GuardId::new("admin"))),
        )?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, forge::Model)]
#[forge(model = AUDIT_ENTRIES_TABLE, primary_key_strategy = "manual")]
struct AuditEntry {
    id: i64,
    title: String,
    #[forge(audit_exclude)]
    secret: String,
    created_at: DateTime,
    updated_at: DateTime,
    deleted_at: Option<DateTime>,
}

fn audit_routes(registrar: &mut HttpRegistrar) -> Result<()> {
    registrar.route_with_options(
        "/audit-entry",
        post(create_audit_entry),
        HttpRouteOptions::new().guard(GuardId::new("admin")),
    );
    Ok(())
}

async fn create_audit_entry(
    State(app): State<AppContext>,
    _actor: CurrentActor,
) -> impl IntoResponse {
    AuditEntry::create()
        .set(AuditEntry::ID, 101_i64)
        .set(AuditEntry::TITLE, "Created over HTTP")
        .set(AuditEntry::SECRET, "never-log-this")
        .save(&app)
        .await
        .unwrap();

    StatusCode::CREATED
}

async fn execute_batch(database: &DatabaseManager, statements: &[&str]) {
    for statement in statements {
        database.raw_execute(statement, &[]).await.unwrap();
    }
}

async fn reset_schema(database: &DatabaseManager) {
    execute_batch(
        database,
        &[
            &format!("DROP TABLE IF EXISTS {AUDIT_ENTRIES_TABLE}"),
            &format!("DROP TABLE IF EXISTS {AUDIT_LOGS_TABLE}"),
            &format!(
                "CREATE TABLE {AUDIT_LOGS_TABLE} (
                    id UUID PRIMARY KEY DEFAULT uuidv7(),
                    event_type TEXT NOT NULL,
                    subject_model TEXT NOT NULL,
                    subject_table TEXT NOT NULL,
                    subject_id TEXT NOT NULL,
                    actor_guard TEXT,
                    actor_id TEXT,
                    request_id TEXT,
                    ip TEXT,
                    user_agent TEXT,
                    before_data JSONB,
                    after_data JSONB,
                    changes JSONB,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                )"
            ),
            &format!(
                "CREATE TABLE {AUDIT_ENTRIES_TABLE} (
                    id BIGINT PRIMARY KEY,
                    title TEXT NOT NULL,
                    secret TEXT NOT NULL,
                    created_at TIMESTAMPTZ NOT NULL,
                    updated_at TIMESTAMPTZ NOT NULL,
                    deleted_at TIMESTAMPTZ NULL
                )"
            ),
        ],
    )
    .await;
}

async fn audit_logs_for_subject<E>(executor: &E, subject_id: i64) -> Vec<AuditLog>
where
    E: QueryExecutor,
{
    AuditLog::query()
        .where_(AuditLog::SUBJECT_TABLE.eq(AUDIT_ENTRIES_TABLE))
        .where_(AuditLog::SUBJECT_ID.eq(subject_id.to_string()))
        .order_by(AuditLog::CREATED_AT.asc())
        .order_by(AuditLog::ID.asc())
        .get(executor)
        .await
        .unwrap()
        .into_vec()
}

async fn latest_audit_log<E>(executor: &E, subject_id: i64) -> AuditLog
where
    E: QueryExecutor,
{
    audit_logs_for_subject(executor, subject_id)
        .await
        .into_iter()
        .last()
        .unwrap()
}

fn json_object(value: &serde_json::Value) -> &serde_json::Map<String, serde_json::Value> {
    value.as_object().unwrap()
}

#[tokio::test]
async fn audit_rows_commit_and_rollback_with_parent_transaction() {
    let _guard = audit_lock().await;
    let Some(runtime) = AuditRuntime::new().await else {
        return;
    };

    reset_schema(runtime.database.as_ref()).await;

    let rollback_tx = runtime.app.begin_transaction().await.unwrap();
    AuditEntry::create()
        .set(AuditEntry::ID, 1_i64)
        .set(AuditEntry::TITLE, "Rollback me")
        .set(AuditEntry::SECRET, "rollback-secret")
        .save(&rollback_tx)
        .await
        .unwrap();
    assert_eq!(audit_logs_for_subject(&rollback_tx, 1).await.len(), 1);
    rollback_tx.rollback().await.unwrap();
    assert!(audit_logs_for_subject(&runtime.app, 1).await.is_empty());

    let commit_tx = runtime.app.begin_transaction().await.unwrap();
    AuditEntry::create()
        .set(AuditEntry::ID, 2_i64)
        .set(AuditEntry::TITLE, "Keep me")
        .set(AuditEntry::SECRET, "commit-secret")
        .save(&commit_tx)
        .await
        .unwrap();
    assert_eq!(audit_logs_for_subject(&commit_tx, 2).await.len(), 1);
    commit_tx.commit().await.unwrap();

    let committed = audit_logs_for_subject(&runtime.app, 2).await;
    assert_eq!(committed.len(), 1);
    assert_eq!(committed[0].event_type, "created");
}

#[tokio::test]
async fn http_writes_capture_actor_and_request_origin() {
    let _guard = audit_lock().await;
    let Some(runtime) = AuditRuntime::new().await else {
        return;
    };

    let app = TestApp::builder()
        .load_config_dir(runtime.config_dir())
        .register_provider(AuditAuthProvider)
        .register_middleware(TrustedProxy::new().build())
        .register_routes(audit_routes)
        .build()
        .await;

    reset_schema(app.app().database().unwrap().as_ref()).await;

    let response = app
        .client()
        .post("/audit-entry")
        .bearer_auth("admin-token")
        .header(REQUEST_ID_HEADER, "req-audit-http")
        .header("x-forwarded-for", "203.0.113.5, 10.0.0.1")
        .header("user-agent", "ForgeAuditAcceptance/1.0")
        .send()
        .await;

    assert_eq!(response.status(), StatusCode::CREATED);

    let log = latest_audit_log(app.app(), 101).await;
    assert_eq!(log.event_type, "created");
    assert_eq!(log.subject_table, AUDIT_ENTRIES_TABLE);
    assert_eq!(log.actor_guard.as_deref(), Some("admin"));
    assert_eq!(log.actor_id.as_deref(), Some("admin-1"));
    assert_eq!(log.request_id.as_deref(), Some("req-audit-http"));
    assert_eq!(log.ip.as_deref(), Some("203.0.113.5"));
    assert_eq!(log.user_agent.as_deref(), Some("ForgeAuditAcceptance/1.0"));

    let after = log.after_data.unwrap();
    assert_eq!(after["title"], "Created over HTTP");
    assert!(json_object(&after).get("secret").is_none());
}

#[tokio::test]
async fn direct_writes_leave_origin_empty_and_track_event_types() {
    let _guard = audit_lock().await;
    let Some(runtime) = AuditRuntime::new().await else {
        return;
    };

    reset_schema(runtime.database.as_ref()).await;

    AuditEntry::create()
        .set(AuditEntry::ID, 10_i64)
        .set(AuditEntry::TITLE, "Draft")
        .set(AuditEntry::SECRET, "hidden-1")
        .save(&runtime.app)
        .await
        .unwrap();

    let created = latest_audit_log(&runtime.app, 10).await;
    assert!(created.actor_guard.is_none());
    assert!(created.actor_id.is_none());
    assert!(created.request_id.is_none());
    assert!(created.ip.is_none());
    assert!(created.user_agent.is_none());

    AuditEntry::update()
        .where_(AuditEntry::ID.eq(10_i64))
        .set(AuditEntry::TITLE, "Published")
        .set(AuditEntry::SECRET, "hidden-2")
        .save(&runtime.app)
        .await
        .unwrap();

    AuditEntry::delete()
        .where_(AuditEntry::ID.eq(10_i64))
        .execute(&runtime.app)
        .await
        .unwrap();

    AuditEntry::restore()
        .where_(AuditEntry::ID.eq(10_i64))
        .save(&runtime.app)
        .await
        .unwrap();

    AuditEntry::force_delete()
        .where_(AuditEntry::ID.eq(10_i64))
        .execute(&runtime.app)
        .await
        .unwrap();

    let logs = audit_logs_for_subject(&runtime.app, 10).await;
    let event_types = logs
        .iter()
        .map(|log| log.event_type.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        vec!["created", "updated", "soft_deleted", "restored", "deleted"]
    );

    let created_after = logs[0].after_data.as_ref().unwrap();
    assert_eq!(created_after["title"], "Draft");
    assert!(json_object(created_after).get("secret").is_none());

    let updated_log = &logs[1];
    let updated_before = updated_log.before_data.as_ref().unwrap();
    let updated_after = updated_log.after_data.as_ref().unwrap();
    let updated_changes = updated_log.changes.as_ref().unwrap();
    assert_eq!(updated_before["title"], "Draft");
    assert_eq!(updated_after["title"], "Published");
    assert_eq!(updated_changes["title"]["before"], "Draft");
    assert_eq!(updated_changes["title"]["after"], "Published");
    assert!(json_object(updated_before).get("secret").is_none());
    assert!(json_object(updated_after).get("secret").is_none());
    assert!(json_object(updated_changes).get("secret").is_none());

    let soft_deleted_log = &logs[2];
    let soft_deleted_changes = soft_deleted_log.changes.as_ref().unwrap();
    assert_eq!(soft_deleted_log.event_type, "soft_deleted");
    assert!(json_object(soft_deleted_changes).contains_key("deleted_at"));

    let restored_log = &logs[3];
    let restored_changes = restored_log.changes.as_ref().unwrap();
    assert_eq!(restored_log.event_type, "restored");
    assert_eq!(
        restored_changes["deleted_at"]["after"],
        serde_json::Value::Null
    );

    let deleted_log = &logs[4];
    assert_eq!(deleted_log.event_type, "deleted");
    assert!(deleted_log.after_data.is_none());
    assert_eq!(
        deleted_log.before_data.as_ref().unwrap()["title"],
        "Published"
    );
}

#[tokio::test]
async fn audit_log_model_does_not_recurse() {
    let _guard = audit_lock().await;
    let Some(runtime) = AuditRuntime::new().await else {
        return;
    };

    reset_schema(runtime.database.as_ref()).await;

    AuditLog::create()
        .set(AuditLog::EVENT_TYPE, "manual")
        .set(AuditLog::SUBJECT_MODEL, "audit_acceptance::Manual")
        .set(AuditLog::SUBJECT_TABLE, "manual_subjects")
        .set(AuditLog::SUBJECT_ID, "manual-1")
        .save(&runtime.app)
        .await
        .unwrap();

    let logs = AuditLog::query()
        .order_by(AuditLog::CREATED_AT.asc())
        .order_by(AuditLog::ID.asc())
        .get(&runtime.app)
        .await
        .unwrap()
        .into_vec();

    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].event_type, "manual");
    assert_eq!(logs[0].subject_table, "manual_subjects");
}
