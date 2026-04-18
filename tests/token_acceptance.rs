use std::fs;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use forge::prelude::*;
use tempfile::TempDir;
use tokio::sync::{Mutex, MutexGuard};

const PAT_TABLE: &str = "personal_access_tokens";

fn postgres_url() -> Option<String> {
    std::env::var("FORGE_TEST_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

async fn token_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

struct TokenTestRuntime {
    _dir: TempDir,
    app: AppContext,
    database: Arc<DatabaseManager>,
}

impl TokenTestRuntime {
    async fn new() -> Option<Self> {
        let url = postgres_url()?;
        let dir = tempfile::tempdir().ok()?;
        fs::write(
            dir.path().join("00-runtime.toml"),
            format!(
                r#"
                [database]
                url = "{url}"
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

        reset_personal_access_tokens(database.as_ref()).await;

        Some(Self {
            _dir: dir,
            app,
            database,
        })
    }

    async fn cleanup(&self) {
        let _ = self
            .database
            .raw_execute(&format!("DROP TABLE IF EXISTS {PAT_TABLE}"), &[])
            .await;
    }

    async fn rows_for_guard(&self, guard: &str) -> Vec<DbRecord> {
        self.database
            .raw_query(
                r#"
                SELECT actor_id, name, access_token_hash, refresh_token_hash,
                       revoked_at IS NULL AS is_active
                FROM personal_access_tokens
                WHERE guard = $1
                ORDER BY created_at, access_token_hash
                "#,
                &[DbValue::Text(guard.to_string())],
            )
            .await
            .unwrap()
    }
}

async fn reset_personal_access_tokens(database: &DatabaseManager) {
    database
        .raw_execute(&format!("DROP TABLE IF EXISTS {PAT_TABLE}"), &[])
        .await
        .unwrap();

    // Keep this in sync with the published PAT migration: actor_id is TEXT.
    database
        .raw_execute(
            r#"
            CREATE TABLE personal_access_tokens (
                id UUID PRIMARY KEY DEFAULT uuidv7(),
                guard TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                name TEXT NOT NULL DEFAULT '',
                access_token_hash TEXT NOT NULL,
                refresh_token_hash TEXT,
                abilities JSONB NOT NULL DEFAULT '[]',
                expires_at TIMESTAMPTZ NOT NULL,
                refresh_expires_at TIMESTAMPTZ,
                last_used_at TIMESTAMPTZ,
                revoked_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
            &[],
        )
        .await
        .unwrap();

    database
        .raw_execute(
            "CREATE INDEX idx_pat_access_hash ON personal_access_tokens (access_token_hash) WHERE revoked_at IS NULL",
            &[],
        )
        .await
        .unwrap();
    database
        .raw_execute(
            "CREATE INDEX idx_pat_refresh_hash ON personal_access_tokens (refresh_token_hash) WHERE revoked_at IS NULL",
            &[],
        )
        .await
        .unwrap();
    database
        .raw_execute(
            "CREATE INDEX idx_pat_actor ON personal_access_tokens (guard, actor_id)",
            &[],
        )
        .await
        .unwrap();
}

#[derive(Clone, Debug)]
struct DirectManagerActor;

impl Model for DirectManagerActor {
    type Lifecycle = NoModelLifecycle;

    fn table_meta() -> &'static TableMeta<Self> {
        unimplemented!("token acceptance tests do not query actor model tables")
    }
}

#[async_trait]
impl Authenticatable for DirectManagerActor {
    fn guard() -> GuardId {
        GuardId::new("text_api")
    }
}

#[derive(Clone, Debug)]
struct ExternalActorUser {
    id: i64,
    external_actor_id: String,
}

impl Model for ExternalActorUser {
    type Lifecycle = NoModelLifecycle;

    fn table_meta() -> &'static TableMeta<Self> {
        unimplemented!("token acceptance tests do not query actor model tables")
    }
}

#[async_trait]
impl Authenticatable for ExternalActorUser {
    fn guard() -> GuardId {
        GuardId::new("external_api")
    }
}

impl HasToken for ExternalActorUser {
    fn token_actor_id(&self) -> String {
        self.external_actor_id.clone()
    }
}

#[derive(Clone, Debug)]
struct UuidBackedActorUser {
    id: ModelId<UuidBackedActorUser>,
    _email: String,
}

impl Model for UuidBackedActorUser {
    type Lifecycle = NoModelLifecycle;

    fn table_meta() -> &'static TableMeta<Self> {
        static COLUMNS: [ColumnInfo; 2] = [
            ColumnInfo::new("id", DbType::Uuid),
            ColumnInfo::new("email", DbType::Text),
        ];
        static TABLE: OnceLock<TableMeta<UuidBackedActorUser>> = OnceLock::new();
        TABLE.get_or_init(|| {
            TableMeta::new(
                "token_uuid_actor_users",
                &COLUMNS,
                "id",
                ModelPrimaryKeyStrategy::Manual,
                ModelBehavior::new(ModelFeatureSetting::Default, ModelFeatureSetting::Default),
                |record| {
                    Ok(UuidBackedActorUser {
                        id: record.decode("id")?,
                        _email: record.decode("email")?,
                    })
                },
            )
        })
    }
}

#[async_trait]
impl Authenticatable for UuidBackedActorUser {
    fn guard() -> GuardId {
        GuardId::new("uuid_api")
    }
}

impl HasToken for UuidBackedActorUser {
    fn token_actor_id(&self) -> String {
        self.id.to_string()
    }
}

#[tokio::test]
async fn token_manager_issue_refresh_and_revoke_all_use_text_actor_ids() {
    let _guard = token_lock().await;
    let Some(runtime) = TokenTestRuntime::new().await else {
        return;
    };

    let tokens = runtime.app.tokens().unwrap();
    let pair = tokens
        .issue_named::<DirectManagerActor>("acct-42", "cli")
        .await
        .unwrap();

    let actor = tokens.validate(&pair.access_token).await.unwrap().unwrap();
    assert_eq!(actor.id, "acct-42");
    assert_eq!(actor.guard, DirectManagerActor::guard());

    let initial_rows = runtime.rows_for_guard("text_api").await;
    assert_eq!(initial_rows.len(), 1);
    assert_eq!(
        initial_rows[0].decode::<String>("actor_id").unwrap(),
        "acct-42"
    );
    assert_eq!(initial_rows[0].decode::<String>("name").unwrap(), "cli");
    assert!(initial_rows[0].decode::<bool>("is_active").unwrap());

    let refreshed = tokens.refresh(&pair.refresh_token).await.unwrap();

    assert!(tokens.validate(&pair.access_token).await.unwrap().is_none());
    let refreshed_actor = tokens
        .validate(&refreshed.access_token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed_actor.id, "acct-42");

    let refreshed_rows = runtime.rows_for_guard("text_api").await;
    assert_eq!(refreshed_rows.len(), 2);
    assert_eq!(
        refreshed_rows
            .iter()
            .filter(|row| row.decode::<bool>("is_active").unwrap())
            .count(),
        1
    );

    let revoked = tokens
        .revoke_all::<DirectManagerActor>("acct-42")
        .await
        .unwrap();
    assert_eq!(revoked, 1);
    assert!(tokens
        .validate(&refreshed.access_token)
        .await
        .unwrap()
        .is_none());

    runtime.cleanup().await;
}

#[tokio::test]
async fn has_token_uses_custom_token_actor_id_for_storage_and_revocation() {
    let _guard = token_lock().await;
    let Some(runtime) = TokenTestRuntime::new().await else {
        return;
    };

    let user = ExternalActorUser {
        id: 7,
        external_actor_id: "merchant:store-9".to_string(),
    };

    let pair = user
        .create_token_named(&runtime.app, "dashboard")
        .await
        .unwrap();
    let actor = runtime
        .app
        .tokens()
        .unwrap()
        .validate(&pair.access_token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actor.id, user.external_actor_id);

    let rows = runtime.rows_for_guard("external_api").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].decode::<String>("actor_id").unwrap(),
        "merchant:store-9"
    );
    assert_ne!(
        rows[0].decode::<String>("actor_id").unwrap(),
        user.id.to_string()
    );

    let revoked = user.revoke_all_tokens(&runtime.app).await.unwrap();
    assert_eq!(revoked, 1);
    assert!(runtime
        .app
        .tokens()
        .unwrap()
        .validate(&pair.access_token)
        .await
        .unwrap()
        .is_none());

    runtime.cleanup().await;
}

#[tokio::test]
async fn uuid_backed_authenticatables_store_actor_ids_as_text() {
    let _guard = token_lock().await;
    let Some(runtime) = TokenTestRuntime::new().await else {
        return;
    };

    let user = UuidBackedActorUser {
        id: ModelId::generate(),
        _email: "uuid@example.com".to_string(),
    };
    let actor_id = user.id.to_string();

    let pair = user.create_token(&runtime.app).await.unwrap();
    let actor = runtime
        .app
        .tokens()
        .unwrap()
        .validate(&pair.access_token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actor.id, actor_id);

    let rows = runtime.rows_for_guard("uuid_api").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].decode::<String>("actor_id").unwrap(), actor_id);
    assert!(rows[0].decode::<bool>("is_active").unwrap());

    let revoked = user.revoke_all_tokens(&runtime.app).await.unwrap();
    assert_eq!(revoked, 1);
    assert!(runtime
        .app
        .tokens()
        .unwrap()
        .validate(&pair.access_token)
        .await
        .unwrap()
        .is_none());

    runtime.cleanup().await;
}
