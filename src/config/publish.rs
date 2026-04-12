use std::path::Path;
use std::sync::Arc;

use crate::cli::CommandRegistrar;
use crate::foundation::Error;
use crate::support::CommandId;

const CONFIG_PUBLISH_COMMAND: CommandId = CommandId::new("config:publish");
const KEY_GENERATE_COMMAND: CommandId = CommandId::new("key:generate");

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

# -----------------------------------------------------------------------------
# Logging
# -----------------------------------------------------------------------------
[logging]
# level = "info"                      # trace, debug, info, warn, error
# format = "json"                     # "json" or "text"
# log_dir = "logs"

# -----------------------------------------------------------------------------
# Observability (Dashboard & Tracing)
# -----------------------------------------------------------------------------
[observability]
# base_path = "/_forge"               # Dashboard route prefix
# tracing_enabled = false             # Enable OpenTelemetry distributed tracing
# otlp_endpoint = "http://localhost:4317"
# service_name = "forge"

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

        Ok(())
    })
}
