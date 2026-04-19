use std::path::Path;
use std::sync::Arc;

use crate::cli::CommandRegistrar;
use crate::foundation::Error;
use crate::support::CommandId;

const ENV_PUBLISH_COMMAND: CommandId = CommandId::new("env:publish");

pub(crate) fn env_publish_cli_registrar() -> CommandRegistrar {
    Arc::new(|registry| {
        registry.command(
            ENV_PUBLISH_COMMAND,
            clap::Command::new(ENV_PUBLISH_COMMAND.as_str().to_string())
                .about("Publish a .env.example file with all available environment variables")
                .arg(
                    clap::Arg::new("path")
                        .long("path")
                        .value_name("DIR")
                        .default_value(".")
                        .help("Directory to write the .env.example file to"),
                )
                .arg(
                    clap::Arg::new("force")
                        .long("force")
                        .action(clap::ArgAction::SetTrue)
                        .help("Overwrite existing .env.example file"),
                ),
            |invocation| async move {
                let dir = invocation
                    .matches()
                    .get_one::<String>("path")
                    .map(|s| s.as_str())
                    .unwrap_or(".");
                let force = invocation.matches().get_flag("force");

                let path = Path::new(dir);
                if !path.exists() {
                    std::fs::create_dir_all(path).map_err(Error::other)?;
                }

                let file_path = path.join(".env.example");
                if file_path.exists() && !force {
                    println!(
                        ".env.example already exists at {}. Use --force to overwrite.",
                        file_path.display()
                    );
                    return Ok(());
                }

                std::fs::write(&file_path, sample_env()).map_err(Error::other)?;
                println!("Published .env.example to {}", file_path.display());

                Ok(())
            },
        )?;
        Ok(())
    })
}

fn sample_env() -> String {
    r#"# =============================================================================
# Forge Framework — Environment Variables
#
# All configuration values can be overridden via environment variables using
# double-underscore notation: SECTION__KEY=value
#
# Nested config: AUTH__TOKENS__ACCESS_TOKEN_TTL_MINUTES=30
# Boolean values: true / false
# Integer values: 3000
#
# Copy this file to .env and fill in your values:
#   cp .env.example .env
# =============================================================================

# -----------------------------------------------------------------------------
# Application
# -----------------------------------------------------------------------------
APP__NAME=my-app
APP__ENVIRONMENT=development
# APP__TIMEZONE=UTC
# APP__SIGNING_KEY=                         # Base64 key — generate with: cargo run -- key:generate

# -----------------------------------------------------------------------------
# HTTP Server
# -----------------------------------------------------------------------------
SERVER__HOST=127.0.0.1
SERVER__PORT=3000

# -----------------------------------------------------------------------------
# Database (PostgreSQL)
# -----------------------------------------------------------------------------
DATABASE__URL=postgres://forge:secret@127.0.0.1:5432/forge
# DATABASE__READ_URL=                       # Read replica (routes reads when set)
# DATABASE__SCHEMA=public
# DATABASE__MIGRATION_TABLE=forge_migrations
# DATABASE__MIGRATIONS_PATH=database/migrations
# DATABASE__SEEDERS_PATH=database/seeders
# DATABASE__MIN_CONNECTIONS=1
# DATABASE__MAX_CONNECTIONS=10
# DATABASE__ACQUIRE_TIMEOUT_MS=5000
# DATABASE__DEFAULT_PER_PAGE=15
# DATABASE__LOG_QUERIES=false
# DATABASE__SLOW_QUERY_THRESHOLD_MS=500
# DATABASE__IDLE_TIMEOUT_SECONDS=600
# DATABASE__MAX_LIFETIME_SECONDS=1800

# Database Model Defaults
# DATABASE__MODELS__TIMESTAMPS_DEFAULT=true
# DATABASE__MODELS__SOFT_DELETES_DEFAULT=false

# -----------------------------------------------------------------------------
# Redis
# -----------------------------------------------------------------------------
REDIS__URL=redis://127.0.0.1/
# REDIS__NAMESPACE=forge                    # Key prefix — derived from app.name:app.environment if not set

# -----------------------------------------------------------------------------
# Authentication
# -----------------------------------------------------------------------------
# AUTH__DEFAULT_GUARD=api
# AUTH__BEARER_PREFIX=Bearer

# Token Settings
# AUTH__TOKENS__ACCESS_TOKEN_TTL_MINUTES=15
# AUTH__TOKENS__REFRESH_TOKEN_TTL_DAYS=30
# AUTH__TOKENS__TOKEN_LENGTH=32
# AUTH__TOKENS__ROTATE_REFRESH_TOKENS=true

# Session Settings
# AUTH__SESSIONS__TTL_MINUTES=120
# AUTH__SESSIONS__COOKIE_NAME=forge_session
# AUTH__SESSIONS__COOKIE_SECURE=true
# AUTH__SESSIONS__COOKIE_PATH=/
# AUTH__SESSIONS__SLIDING_EXPIRY=true
# AUTH__SESSIONS__REMEMBER_TTL_DAYS=30

# Guard Drivers (per guard name)
# AUTH__GUARDS__API__DRIVER=token
# AUTH__GUARDS__WEB__DRIVER=session

# -----------------------------------------------------------------------------
# Cache
# -----------------------------------------------------------------------------
# CACHE__DRIVER=redis                       # "redis" or "memory"
# CACHE__PREFIX=cache:
# CACHE__TTL_SECONDS=3600
# CACHE__MAX_ENTRIES=10000

# -----------------------------------------------------------------------------
# Background Jobs
# -----------------------------------------------------------------------------
# JOBS__QUEUE=default
# JOBS__MAX_RETRIES=5
# JOBS__POLL_INTERVAL_MS=100
# JOBS__LEASE_TTL_MS=30000
# JOBS__REQUEUE_BATCH_SIZE=64
# JOBS__MAX_CONCURRENT_JOBS=0               # 0 = unlimited
# JOBS__TIMEOUT_SECONDS=300
# JOBS__TRACK_HISTORY=true

# -----------------------------------------------------------------------------
# Scheduler
# -----------------------------------------------------------------------------
# SCHEDULER__TICK_INTERVAL_MS=1000
# SCHEDULER__LEADER_LEASE_TTL_MS=5000

# -----------------------------------------------------------------------------
# WebSocket
# -----------------------------------------------------------------------------
# WEBSOCKET__HOST=127.0.0.1
# WEBSOCKET__PORT=3010
# WEBSOCKET__PATH=/ws
# WEBSOCKET__HEARTBEAT_INTERVAL_SECONDS=30
# WEBSOCKET__HEARTBEAT_TIMEOUT_SECONDS=10
# WEBSOCKET__MAX_MESSAGES_PER_SECOND=50
# WEBSOCKET__MAX_CONNECTIONS_PER_USER=5

# -----------------------------------------------------------------------------
# Email
# -----------------------------------------------------------------------------
# EMAIL__DEFAULT=smtp                       # Default mailer name
# EMAIL__QUEUE=default                      # Queue for async email dispatch
# EMAIL__TEMPLATE_PATH=templates/emails

# From Address
# EMAIL__FROM__ADDRESS=
# EMAIL__FROM__NAME=

# SMTP Mailer
# EMAIL__MAILERS__SMTP__HOST=smtp.example.com
# EMAIL__MAILERS__SMTP__PORT=587
# EMAIL__MAILERS__SMTP__USERNAME=
# EMAIL__MAILERS__SMTP__PASSWORD=
# EMAIL__MAILERS__SMTP__ENCRYPTION=starttls # "starttls", "tls", or "none"
# EMAIL__MAILERS__SMTP__TIMEOUT_SECS=30

# Amazon SES Mailer
# EMAIL__MAILERS__SES__KEY=
# EMAIL__MAILERS__SES__SECRET=
# EMAIL__MAILERS__SES__REGION=us-east-1

# Postmark Mailer
# EMAIL__MAILERS__POSTMARK__SERVER_TOKEN=

# Resend Mailer
# EMAIL__MAILERS__RESEND__API_KEY=

# Mailgun Mailer
# EMAIL__MAILERS__MAILGUN__DOMAIN=
# EMAIL__MAILERS__MAILGUN__API_KEY=
# EMAIL__MAILERS__MAILGUN__REGION=us        # "us" or "eu"

# Log Mailer (development — logs instead of sending)
# EMAIL__MAILERS__LOG__TARGET=email.outbound

# -----------------------------------------------------------------------------
# Storage
# -----------------------------------------------------------------------------
# STORAGE__DEFAULT=local

# Local Disk
# STORAGE__DISKS__LOCAL__DRIVER=local
# STORAGE__DISKS__LOCAL__ROOT=storage/app
# STORAGE__DISKS__LOCAL__URL=/storage
# STORAGE__DISKS__LOCAL__VISIBILITY=private  # "public" or "private"

# S3-Compatible Disk
# STORAGE__DISKS__S3__DRIVER=s3
# STORAGE__DISKS__S3__BUCKET=
# STORAGE__DISKS__S3__REGION=
# STORAGE__DISKS__S3__KEY=
# STORAGE__DISKS__S3__SECRET=
# STORAGE__DISKS__S3__ENDPOINT=             # Custom endpoint for MinIO, R2, etc.
# STORAGE__DISKS__S3__URL=
# STORAGE__DISKS__S3__USE_PATH_STYLE=false
# STORAGE__DISKS__S3__VISIBILITY=private

# -----------------------------------------------------------------------------
# Encryption
# -----------------------------------------------------------------------------
# CRYPT__KEY=                               # Base64 key — generate with: cargo run -- key:generate

# -----------------------------------------------------------------------------
# Hashing
# -----------------------------------------------------------------------------
# HASHING__DRIVER=argon2
# HASHING__MEMORY_COST=19456
# HASHING__TIME_COST=2
# HASHING__PARALLELISM=1

# -----------------------------------------------------------------------------
# Logging
# -----------------------------------------------------------------------------
# LOGGING__LEVEL=info                       # trace, debug, info, warn, error
# LOGGING__FORMAT=json                      # "json" or "text"
# LOGGING__LOG_DIR=logs
# LOGGING__RETENTION_DAYS=30

# -----------------------------------------------------------------------------
# Observability
# -----------------------------------------------------------------------------
# OBSERVABILITY__BASE_PATH=/_forge
# OBSERVABILITY__TRACING_ENABLED=false
# OBSERVABILITY__OTLP_ENDPOINT=http://localhost:4317
# OBSERVABILITY__SERVICE_NAME=forge
# OBSERVABILITY__WEBSOCKET__INCLUDE_PAYLOADS=false

# -----------------------------------------------------------------------------
# Internationalization
# -----------------------------------------------------------------------------
# I18N__DEFAULT_LOCALE=en
# I18N__FALLBACK_LOCALE=en
# I18N__RESOURCE_PATH=locales
"#
    .to_string()
}
