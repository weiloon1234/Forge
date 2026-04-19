use std::path::Path;
use std::sync::Arc;

use crate::cli::CommandRegistrar;
use crate::foundation::{Error, Result};
use crate::support::CommandId;

const CONFIG_PUBLISH_COMMAND: CommandId = CommandId::new("config:publish");
const KEY_GENERATE_COMMAND: CommandId = CommandId::new("key:generate");
const MIGRATE_PUBLISH_COMMAND: CommandId = CommandId::new("migrate:publish");
const SEED_PUBLISH_COMMAND: CommandId = CommandId::new("seed:publish");
const SEED_COUNTRIES_COMMAND: CommandId = CommandId::new("seed:countries");
const ABOUT_COMMAND: CommandId = CommandId::new("about");

/// Generate the full sample configuration TOML.
///
/// Required fields are uncommented; optional fields are commented out with
/// their default values so users can uncomment what they need.
pub fn sample_config() -> String {
    r#"# =============================================================================
# Forge Framework Configuration
#
# This file contains all available configuration options with their defaults.
# Required fields are uncommented. Optional fields are commented out — uncomment
# and modify as needed.
#
# Environment variable overlay: any key can be overridden via env vars using
# double-underscore notation. Example: DATABASE__URL=postgres://...
# =============================================================================

# -----------------------------------------------------------------------------
# Application
# -----------------------------------------------------------------------------
[app]
name = "my-app"                       # App name — used in Redis key prefix for multi-project safety
environment = "development"           # "development", "production", or "testing"
timezone = "UTC"
# signing_key = ""                    # Base64-encoded key for signed routes — generate with `key:generate`

# -----------------------------------------------------------------------------
# HTTP Server
# -----------------------------------------------------------------------------
[server]
host = "127.0.0.1"
port = 3000

# -----------------------------------------------------------------------------
# Redis
# -----------------------------------------------------------------------------
[redis]
url = "redis://127.0.0.1/"
# namespace = "forge"                 # Key prefix — auto-derived from app.name:app.environment if not set

# -----------------------------------------------------------------------------
# Database (PostgreSQL)
# -----------------------------------------------------------------------------
[database]
url = "postgres://forge:secret@127.0.0.1:5432/forge"
# read_url = ""                       # Read replica URL (auto-routes reads when set)
# schema = "public"
# migration_table = "forge_migrations"
# migrations_path = "database/migrations"
# seeders_path = "database/seeders"
# min_connections = 1
# max_connections = 10
# acquire_timeout_ms = 5000
# default_per_page = 15               # Default pagination page size
# log_queries = false                  # Log all SQL queries to tracing (dev only)
# slow_query_threshold_ms = 500       # Log queries exceeding this threshold
# idle_timeout_seconds = 600          # Close idle connections after 10 min
# max_lifetime_seconds = 1800         # Recycle connections after 30 min

[database.models]
# timestamps_default = true           # Auto-add created_at/updated_at
# soft_deletes_default = false         # Auto-add deleted_at

# -----------------------------------------------------------------------------
# Authentication
# -----------------------------------------------------------------------------
[auth]
# default_guard = "api"
# bearer_prefix = "Bearer"

[auth.tokens]
# access_token_ttl_minutes = 15
# refresh_token_ttl_days = 30
# token_length = 32
# rotate_refresh_tokens = true

[auth.sessions]
# ttl_minutes = 120
# cookie_name = "forge_session"
# cookie_secure = true
# cookie_path = "/"
# sliding_expiry = true
# remember_ttl_days = 30

# Define guards (one per authentication portal):
# [auth.guards.api]
# driver = "token"                    # "token", "session", or "custom"
#
# [auth.guards.web]
# driver = "session"

# -----------------------------------------------------------------------------
# Jobs (Background Queue)
# -----------------------------------------------------------------------------
[jobs]
# queue = "default"
# max_retries = 5
# poll_interval_ms = 100
# lease_ttl_ms = 30000
# requeue_batch_size = 64
# max_concurrent_jobs = 0              # 0 = unlimited (goroutine-style), or set a limit
# timeout_seconds = 300
# track_history = true

# Queue priorities (lower number = claimed first):
# [jobs.queue_priorities]
# high = 1
# default = 5
# low = 10

# -----------------------------------------------------------------------------
# Scheduler (Cron)
# -----------------------------------------------------------------------------
[scheduler]
# tick_interval_ms = 1000
# leader_lease_ttl_ms = 5000

# -----------------------------------------------------------------------------
# WebSocket
# -----------------------------------------------------------------------------
[websocket]
# host = "127.0.0.1"
# port = 3010
# path = "/ws"
# heartbeat_interval_seconds = 30
# heartbeat_timeout_seconds = 10
# max_messages_per_second = 50
# max_connections_per_user = 5
# history_ttl_seconds = 604800          # Auto-reap ws:history:<channel> after 7 days idle (0 = never)

# -----------------------------------------------------------------------------
# Logging
# -----------------------------------------------------------------------------
[logging]
# level = "info"                      # trace, debug, info, warn, error
# format = "json"                     # "json" or "text"
# log_dir = "logs"
# retention_days = 30                 # Auto-delete logs older than N days (0 = keep forever)

# -----------------------------------------------------------------------------
# Observability (Dashboard & Tracing)
# -----------------------------------------------------------------------------
[observability]
# base_path = "/_forge"               # Dashboard route prefix
# tracing_enabled = false             # Enable OpenTelemetry distributed tracing
# otlp_endpoint = "http://localhost:4317"
# service_name = "forge"

# [observability.websocket]
# include_payloads = false         # Include full payloads in /_forge/ws/history/:channel

# -----------------------------------------------------------------------------
# Cache
# -----------------------------------------------------------------------------
[cache]
# driver = "redis"                    # "redis" or "memory"
# prefix = "cache:"
# ttl_seconds = 3600
# max_entries = 10000                 # Max entries for memory driver

# -----------------------------------------------------------------------------
# Hashing (Password)
# -----------------------------------------------------------------------------
[hashing]
# driver = "argon2"
# memory_cost = 19456
# time_cost = 2
# parallelism = 1

# -----------------------------------------------------------------------------
# Encryption
# -----------------------------------------------------------------------------
[crypt]
# key = ""                            # Base64-encoded 256-bit key for AES-256-GCM

# -----------------------------------------------------------------------------
# Internationalization
# -----------------------------------------------------------------------------
[i18n]
# default_locale = "en"
# fallback_locale = "en"
# resource_path = "locales"

# -----------------------------------------------------------------------------
# Email
# -----------------------------------------------------------------------------
[email]
# default = "smtp"                    # Default mailer: smtp, ses, postmark, resend, mailgun, log
# queue = "default"                   # Queue name for queued email delivery
# template_path = "templates/emails"

[email.from]
# address = ""
# name = ""

# SMTP mailer:
# [email.mailers.smtp]
# host = "smtp.example.com"
# port = 587
# username = ""
# password = ""
# encryption = "starttls"             # "starttls", "tls", or "none"
# timeout_secs = 30

# Amazon SES mailer:
# [email.mailers.ses]
# key = ""
# secret = ""
# region = "us-east-1"

# Postmark mailer:
# [email.mailers.postmark]
# server_token = ""

# Resend mailer:
# [email.mailers.resend]
# api_key = ""

# Mailgun mailer:
# [email.mailers.mailgun]
# domain = ""
# api_key = ""
# region = "us"                       # "us" or "eu"

# Log mailer (development — logs instead of sending):
# [email.mailers.log]
# target = "email.outbound"

# -----------------------------------------------------------------------------
# Storage (File System)
# -----------------------------------------------------------------------------
[storage]
# default = "local"

# Local disk:
# [storage.disks.local]
# driver = "local"
# root = "storage/app"
# url = "/storage"                    # Public URL prefix (optional)
# visibility = "private"             # "public" or "private"

# S3-compatible disk:
# [storage.disks.s3]
# driver = "s3"
# bucket = ""
# region = ""
# key = ""
# secret = ""
# endpoint = ""                       # Custom endpoint for MinIO, R2, etc.
# url = ""                            # Public URL prefix (optional)
# use_path_style = false
# visibility = "private"
"#
    .to_string()
}

pub(crate) fn config_publish_cli_registrar() -> CommandRegistrar {
    Arc::new(|registry| {
        registry.command(
            CONFIG_PUBLISH_COMMAND,
            clap::Command::new(CONFIG_PUBLISH_COMMAND.as_str().to_string())
                .about("Publish a sample configuration file to the config directory")
                .arg(
                    clap::Arg::new("path")
                        .long("path")
                        .value_name("DIR")
                        .default_value("config")
                        .help("Directory to write the config file to"),
                )
                .arg(
                    clap::Arg::new("force")
                        .long("force")
                        .action(clap::ArgAction::SetTrue)
                        .help("Overwrite existing config file"),
                ),
            |invocation| async move {
                let dir = invocation
                    .matches()
                    .get_one::<String>("path")
                    .map(|s| s.as_str())
                    .unwrap_or("config");
                let force = invocation.matches().get_flag("force");

                let path = Path::new(dir);
                if !path.exists() {
                    std::fs::create_dir_all(path).map_err(Error::other)?;
                }

                let file_path = path.join("forge.toml");
                if file_path.exists() && !force {
                    println!(
                        "Config file already exists at {}. Use --force to overwrite.",
                        file_path.display()
                    );
                    return Ok(());
                }

                std::fs::write(&file_path, sample_config()).map_err(Error::other)?;
                println!("Configuration published to {}", file_path.display());

                Ok(())
            },
        )?;

        registry.command(
            KEY_GENERATE_COMMAND,
            clap::Command::new(KEY_GENERATE_COMMAND.as_str().to_string())
                .about("Generate application keys (signing key and encryption key)"),
            |_invocation| async move {
                use base64::{engine::general_purpose::STANDARD, Engine};

                let signing_key = STANDARD.encode(crate::support::Token::bytes(32)?);
                let crypt_key = STANDARD.encode(crate::support::Token::bytes(32)?);

                println!("Keys generated successfully.\n");
                println!("Add to your config file:\n");
                println!("  [app]");
                println!("  signing_key = \"{signing_key}\"\n");
                println!("  [crypt]");
                println!("  key = \"{crypt_key}\"\n");
                println!("Or set via environment variables:\n");
                println!("  APP__SIGNING_KEY={signing_key}");
                println!("  CRYPT__KEY={crypt_key}");

                Ok(())
            },
        )?;

        registry.command(
            MIGRATE_PUBLISH_COMMAND,
            clap::Command::new(MIGRATE_PUBLISH_COMMAND.as_str().to_string())
                .about("Publish framework migration files to your project")
                .arg(
                    clap::Arg::new("path")
                        .long("path")
                        .value_name("DIR")
                        .default_value("database/migrations")
                        .help("Directory to write migration files to"),
                )
                .arg(
                    clap::Arg::new("force")
                        .long("force")
                        .action(clap::ArgAction::SetTrue)
                        .help("Overwrite existing migration files"),
                ),
            |invocation| async move {
                let dir = invocation
                    .matches()
                    .get_one::<String>("path")
                    .map(|s| s.as_str())
                    .unwrap_or("database/migrations");
                let force = invocation.matches().get_flag("force");

                publish_framework_files(dir, FRAMEWORK_MIGRATIONS, force, "migration")?;
                Ok(())
            },
        )?;

        registry.command(
            SEED_PUBLISH_COMMAND,
            clap::Command::new(SEED_PUBLISH_COMMAND.as_str().to_string())
                .about("Publish framework seeder files to your project")
                .arg(
                    clap::Arg::new("path")
                        .long("path")
                        .value_name("DIR")
                        .default_value("database/seeders")
                        .help("Directory to write seeder files to"),
                )
                .arg(
                    clap::Arg::new("force")
                        .long("force")
                        .action(clap::ArgAction::SetTrue)
                        .help("Overwrite existing seeder files"),
                ),
            |invocation| async move {
                let dir = invocation
                    .matches()
                    .get_one::<String>("path")
                    .map(|s| s.as_str())
                    .unwrap_or("database/seeders");
                let force = invocation.matches().get_flag("force");

                publish_framework_files(dir, FRAMEWORK_SEEDERS, force, "seeder")?;
                Ok(())
            },
        )?;

        registry.command(
            SEED_COUNTRIES_COMMAND,
            clap::Command::new(SEED_COUNTRIES_COMMAND.as_str().to_string())
                .about("Seed the countries table with 250 built-in country records"),
            |invocation| async move {
                let app = invocation.app();
                let count = crate::countries::seed_countries(app).await?;
                println!("Seeded {count} countries.");
                Ok(())
            },
        )?;

        registry.command(
            ABOUT_COMMAND,
            clap::Command::new(ABOUT_COMMAND.as_str().to_string())
                .about("Display framework version and environment summary"),
            |invocation| async move {
                let app = invocation.app();
                let config = app.config();

                println!("Forge Framework v{}\n", env!("CARGO_PKG_VERSION"));

                let app_config = config.app().unwrap_or_default();
                println!("  Environment:  {}", app_config.environment);
                println!("  Timezone:     {}", app_config.timezone);

                let signing = if app_config.signing_key.is_empty() {
                    "not configured"
                } else {
                    "configured"
                };
                println!("  Signing key:  {}", signing);

                if let Ok(db) = config.database() {
                    let db_status = if db.url.is_empty() {
                        "not configured"
                    } else {
                        "configured"
                    };
                    println!("  Database:     {}", db_status);
                    if db.read_url.as_deref().is_some_and(|u| !u.is_empty()) {
                        println!("  Read replica: configured");
                    }
                }

                if let Ok(redis) = config.redis() {
                    let redis_status = if redis.url.is_empty() {
                        "not configured"
                    } else {
                        "configured"
                    };
                    println!("  Redis:        {}", redis_status);
                }

                if let Ok(cache) = config.cache() {
                    println!("  Cache:        {:?}", cache.driver);
                }

                if let Ok(logging) = config.logging() {
                    println!("  Log level:    {:?}", logging.level);
                    println!("  Log format:   {:?}", logging.format);
                    println!("  Retention:    {} days", logging.retention_days);
                }

                if let Ok(plugins) = app.resolve::<crate::plugin::PluginRegistry>() {
                    if !plugins.is_empty() {
                        println!("  Plugins:      registered");
                    }
                }

                Ok(())
            },
        )?;

        Ok(())
    })
}

fn publish_framework_files(
    dir: &str,
    files: &[(&'static str, &'static str)],
    force: bool,
    kind: &str,
) -> Result<()> {
    let path = Path::new(dir);
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(Error::other)?;
    }

    let mut published = 0;
    for (name, contents) in files {
        let file_path = path.join(name);
        if file_path.exists() && !force {
            println!("  skip  {} (exists)", name);
            continue;
        }

        std::fs::write(&file_path, contents).map_err(Error::other)?;
        println!("  create  {}", name);
        published += 1;
    }

    if published == 0 {
        println!("\nAll {kind}s already exist. Use --force to overwrite.");
    } else {
        println!("\n{published} {kind}(s) published to {dir}");
    }

    Ok(())
}

/// Framework-provided migration files (Rust format, discoverable by forge-build).
const FRAMEWORK_MIGRATIONS: &[(&str, &str)] = &[
    (
        "000000000001_create_personal_access_tokens.rs",
        include_str!("../../database/migrations/000000000001_create_personal_access_tokens.rs"),
    ),
    (
        "000000000002_create_password_reset_tokens.rs",
        include_str!("../../database/migrations/000000000002_create_password_reset_tokens.rs"),
    ),
    (
        "000000000003_create_notifications.rs",
        include_str!("../../database/migrations/000000000003_create_notifications.rs"),
    ),
    (
        "000000000004_create_job_history.rs",
        include_str!("../../database/migrations/000000000004_create_job_history.rs"),
    ),
    (
        "000000000005_create_attachments.rs",
        include_str!("../../database/migrations/000000000005_create_attachments.rs"),
    ),
    (
        "000000000006_create_metadata.rs",
        include_str!("../../database/migrations/000000000006_create_metadata.rs"),
    ),
    (
        "000000000007_create_model_translations.rs",
        include_str!("../../database/migrations/000000000007_create_model_translations.rs"),
    ),
    (
        "000000000008_create_countries.rs",
        include_str!("../../database/migrations/000000000008_create_countries.rs"),
    ),
    (
        "000000000009_create_settings.rs",
        include_str!("../../database/migrations/000000000009_create_settings.rs"),
    ),
];

/// Framework-provided seeder files (Rust format, discoverable by forge-build).
const FRAMEWORK_SEEDERS: &[(&str, &str)] = &[(
    "000000000001_countries_seeder.rs",
    include_str!("../../database/seeders/000000000001_countries_seeder.rs"),
)];
