use std::fs;
use std::sync::OnceLock;

use forge::prelude::*;
use forge::settings::{NewSetting, Setting};
use tempfile::TempDir;
use tokio::sync::{Mutex, MutexGuard};

fn postgres_url() -> Option<String> {
    std::env::var("FORGE_TEST_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

async fn support_stores_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

struct SupportStoresRuntime {
    _dir: TempDir,
    app: AppContext,
    database: std::sync::Arc<DatabaseManager>,
}

impl SupportStoresRuntime {
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

        reset_settings(database.as_ref()).await;
        reset_notifications(database.as_ref()).await;

        Some(Self {
            _dir: dir,
            app,
            database,
        })
    }

    async fn cleanup(&self) {
        let _ = self
            .database
            .raw_execute("DROP TABLE IF EXISTS settings", &[])
            .await;
        let _ = self
            .database
            .raw_execute("DROP TABLE IF EXISTS notifications", &[])
            .await;
    }
}

async fn reset_settings(database: &DatabaseManager) {
    database
        .raw_execute("DROP TABLE IF EXISTS settings", &[])
        .await
        .unwrap();
    database
        .raw_execute(
            r#"
            CREATE TABLE settings (
                id UUID PRIMARY KEY DEFAULT uuidv7(),
                key TEXT NOT NULL,
                value JSONB,
                setting_type TEXT NOT NULL DEFAULT 'text',
                parameters JSONB NOT NULL DEFAULT '{}',
                group_name TEXT NOT NULL DEFAULT 'general',
                label TEXT NOT NULL DEFAULT '',
                description TEXT,
                sort_order INT NOT NULL DEFAULT 0,
                is_public BOOLEAN NOT NULL DEFAULT false,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ
            )
            "#,
            &[],
        )
        .await
        .unwrap();
    database
        .raw_execute(
            "CREATE UNIQUE INDEX idx_settings_key ON settings (key)",
            &[],
        )
        .await
        .unwrap();
}

async fn reset_notifications(database: &DatabaseManager) {
    database
        .raw_execute("DROP TABLE IF EXISTS notifications", &[])
        .await
        .unwrap();
    database
        .raw_execute(
            r#"
            CREATE TABLE notifications (
                id UUID PRIMARY KEY DEFAULT uuidv7(),
                notifiable_id TEXT NOT NULL,
                type TEXT NOT NULL,
                data JSONB NOT NULL DEFAULT '{}',
                read_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
            &[],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn settings_crud_lists_and_prefix_filters_use_typed_queries() {
    let _guard = support_stores_lock().await;
    let Some(runtime) = SupportStoresRuntime::new().await else {
        return;
    };

    Setting::create(
        &runtime.app,
        NewSetting::new("feature.alpha", "Alpha")
            .value(serde_json::json!(true))
            .group("features")
            .sort_order(2)
            .is_public(true),
    )
    .await
    .unwrap();
    Setting::create(
        &runtime.app,
        NewSetting::new("feature.percent%literal", "Percent Literal").group("features"),
    )
    .await
    .unwrap();
    Setting::create(
        &runtime.app,
        NewSetting::new("feature.percentXliteral", "Percent Wildcard").group("features"),
    )
    .await
    .unwrap();
    Setting::upsert(&runtime.app, "feature.beta", serde_json::json!("beta"))
        .await
        .unwrap();
    Setting::set(&runtime.app, "feature.alpha", serde_json::json!(false))
        .await
        .unwrap();

    assert_eq!(
        Setting::get(&runtime.app, "feature.alpha").await.unwrap(),
        Some(serde_json::json!(false))
    );
    assert!(Setting::exists(&runtime.app, "feature.beta").await.unwrap());
    assert_eq!(
        Setting::find(&runtime.app, "feature.alpha")
            .await
            .unwrap()
            .unwrap()
            .group_name,
        "features"
    );
    assert_eq!(
        Setting::groups(&runtime.app).await.unwrap(),
        vec!["features"]
    );
    assert_eq!(Setting::public(&runtime.app).await.unwrap().len(), 1);
    assert_eq!(
        Setting::by_group(&runtime.app, "features")
            .await
            .unwrap()
            .len(),
        4
    );

    let literal_prefix = Setting::by_prefix(&runtime.app, "feature.percent%")
        .await
        .unwrap();
    assert_eq!(literal_prefix.len(), 1);
    assert_eq!(literal_prefix[0].key, "feature.percent%literal");

    assert!(Setting::remove(&runtime.app, "feature.beta").await.unwrap());
    assert!(!Setting::exists(&runtime.app, "feature.beta").await.unwrap());

    runtime.cleanup().await;
}

struct TestNotifiable;

impl Notifiable for TestNotifiable {
    fn notification_id(&self) -> String {
        "user-42".to_string()
    }
}

struct DatabaseOnlyNotification;

impl Notification for DatabaseOnlyNotification {
    fn notification_type(&self) -> &str {
        "database_only"
    }

    fn via(&self) -> Vec<NotificationChannelId> {
        vec![NOTIFY_DATABASE]
    }

    fn to_database(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({ "message": "stored" }))
    }
}

#[tokio::test]
async fn database_notifications_are_stored_with_typed_queries() {
    let _guard = support_stores_lock().await;
    let Some(runtime) = SupportStoresRuntime::new().await else {
        return;
    };

    runtime
        .app
        .notify(&TestNotifiable, &DatabaseOnlyNotification)
        .await
        .unwrap();

    let rows = runtime
        .database
        .raw_query(
            "SELECT notifiable_id, type, data FROM notifications ORDER BY created_at",
            &[],
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].try_text("notifiable_id").unwrap(), "user-42");
    assert_eq!(rows[0].try_text("type").unwrap(), "database_only");
    assert_eq!(
        rows[0].decode::<serde_json::Value>("data").unwrap(),
        serde_json::json!({ "message": "stored" })
    );

    runtime.cleanup().await;
}
