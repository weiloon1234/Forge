use std::fmt::Write as _;

/// Optional one-line description per top-level module for the index.
/// New modules work without an entry here — they just show no description.
pub(crate) fn module_description(stem: &str) -> &'static str {
    match stem {
        "app_enum" => "Enum metadata and serialization (ForgeAppEnum)",
        "audit" => "Built-in audit logging with automatic model mutation tracking",
        "attachments" => "File attachments with lifecycle (HasAttachments)",
        "auth" => "Auth: guards, policies, tokens, sessions, password reset, email verification",
        "cache" => "In-memory and Redis-backed caching (CacheManager)",
        "cli" => "CLI command registration (CommandRegistry)",
        "config" => "TOML-based configuration (ConfigRepository, AppConfig, etc.)",
        "countries" => "Built-in country data (250 countries)",
        "database" => "AST-first query system: models, relations, projections, compiler",
        "datatable" => "Server-side datatables: filtering, sorting, pagination, XLSX export",
        "email" => "Multi-driver email: SMTP, Mailgun, Postmark, Resend, SES",
        "events" => "Domain event bus with typed listeners",
        "foundation" => "Core: App, AppBuilder, AppContext, AppTransaction, Error, ServiceProvider",
        "http" => "HTTP: routes, middleware (CORS, CSRF, rate limit, etc.), cookies, resources",
        "i18n" => "Internationalization: locale extraction, translation catalogs",
        "imaging" => "Image processing pipeline (resize, crop, rotate, format conversion)",
        "jobs" => "Background job queue with leased at-least-once delivery",
        "kernel" => "5 runtime kernels: HTTP, CLI, Scheduler, Worker, WebSocket",
        "logging" => "Structured logging, observability, health probes, diagnostics",
        "metadata" => "Key-value metadata for models (HasMetadata)",
        "notifications" => "Multi-channel notifications: email, database, broadcast",
        "openapi" => "OpenAPI 3.1.0 spec generation (ApiSchema, RouteDoc)",
        "plugin" => "Compile-time plugin system with dependency validation",
        "redis" => "Namespaced Redis wrapper (RedisManager, RedisConnection)",
        "scheduler" => "Cron + interval scheduling with Redis-safe leadership",
        "storage" => "File storage: local + S3, multipart uploads, file validation",
        "support" => "Utilities: typed IDs, datetime/clock, Collection<T>, crypto, hashing, locks",
        "testing" => "Test infrastructure: TestApp, TestClient, Factory",
        "translations" => "Model field translations across locales (HasTranslations)",
        "validation" => "Validation: 38+ rules, custom rules, request validation extractor",
        "websocket" => "Channel-based WebSocket with presence and typed messages",
        _ => "",
    }
}

pub(crate) fn append_module_notes(group_key: &str, content: &mut String) {
    let notes = module_notes(group_key);
    if notes.is_empty() {
        return;
    }

    writeln!(content, "## Notes").unwrap();
    writeln!(content).unwrap();
    for note in notes {
        writeln!(content, "- {note}").unwrap();
    }
    writeln!(content).unwrap();
}

fn module_notes(group_key: &str) -> &'static [&'static str] {
    match group_key {
        "config" => &[
            "`AppConfig` fields: `name`, `environment`, `timezone`, `signing_key`, `background_shutdown_timeout_ms`.",
            "`SchedulerConfig` includes `shutdown_timeout_ms` for active schedule task draining.",
        ],
        "jobs" => &[
            "`spawn_worker(app)` is managed by the app lifecycle. On app shutdown, Forge asks the worker to drain and aborts it after `app.background_shutdown_timeout_ms`.",
        ],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::{append_module_notes, module_description};

    #[test]
    fn module_descriptions_cover_recent_public_modules() {
        assert_eq!(
            module_description("audit"),
            "Built-in audit logging with automatic model mutation tracking"
        );
    }

    #[test]
    fn module_notes_include_background_shutdown_metadata() {
        let mut config = String::new();
        append_module_notes("config", &mut config);
        assert!(config.contains("background_shutdown_timeout_ms"));
        assert!(config.contains("shutdown_timeout_ms"));

        let mut jobs = String::new();
        append_module_notes("jobs", &mut jobs);
        assert!(jobs.contains("spawn_worker(app)"));
        assert!(jobs.contains("app.background_shutdown_timeout_ms"));
    }
}
