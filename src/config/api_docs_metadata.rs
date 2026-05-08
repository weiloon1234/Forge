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
            "`HttpConfig` is optional and additive: global body cap, request timeout, CORS, CSRF, trusted proxy, and rate limiting are opt-in; security headers are enabled by default with HSTS off.",
            "`DatabaseConfig.migration_lock_timeout_ms` defaults to `0`; `db:migrate` and `db:rollback` wait forever for the migration advisory lock unless overridden.",
            "`JobsConfig` includes `shutdown_timeout_ms` for active worker job draining; `0` aborts active jobs immediately.",
            "`JobsConfig.history_retention_days` defaults to `30`; `0` keeps `job_history` forever.",
            "`ObservabilityConfig.enabled` gates `/_forge/*` route registration; `capture_enabled` gates passive runtime capture.",
            "`SchedulerConfig` includes `shutdown_timeout_ms` for active schedule task draining; `0` aborts active schedules immediately.",
        ],
        "http" => &[
            "`HttpConfig.security_headers` is applied globally by default with HSTS disabled until explicitly enabled.",
            "`HttpConfig.trusted_proxy` honors forwarded client IP headers only from configured CIDRs; code-registered `TrustedProxy::new()` remains compatible and trusts all headers.",
            "Config-derived CORS validates origins, methods, and headers at boot; wildcard origins with credentials are rejected.",
            "Config-derived CSRF is opt-in; code-registered `Csrf` remains source-compatible and path exclusions are segment-aware.",
            "Config-derived body-limit, request-timeout, and rate-limit rejections return JSON `ErrorResponse` bodies with HTTP 413, 408, and 429.",
            "Actor-only rate limits require an authenticated actor; use `actor_or_ip` when a global rate limit needs an IP fallback.",
            "IP rate limits use `TrustedProxy` real IP when available and otherwise fall back to TCP peer connect info on the real server path.",
        ],
        "jobs" => &[
            "`JobsConfig.shutdown_timeout_ms` defaults to `30000`; `0` aborts active jobs immediately on shutdown.",
            "Shutdown-aborted jobs are left unacked so lease expiry and the existing requeue flow make them runnable again.",
            "Job handler panics are handled as normal job failures and use the existing retry/dead-letter flow.",
            "`job_history` is pruned by workers with a distributed lock; consumer apps do not need to register a cleanup scheduler.",
            "`spawn_worker(app)` is managed by the app lifecycle and remains capped by `app.background_shutdown_timeout_ms`.",
        ],
        "logging" => &[
            "`/_forge/runtime` returns the structured `RuntimeSnapshot`; `/_forge/metrics` exposes the same runtime counter families in Prometheus text format.",
            "Forge does not store Prometheus samples; scrape retention belongs to Prometheus or your metrics backend.",
            "`ObservabilityConfig.enabled` controls `/_forge/*` route registration; `capture_enabled` controls passive runtime capture while preserving route availability.",
            "Runtime counters, HTTP samples, SQL slow queries, N+1 suspects, and WebSocket channel counters are bounded process memory and reset on restart.",
            "`/_forge/sql` returns slow-query stats, top-slowest ranking, and potential HTTP N+1 suspects while preserving the existing `slow_queries` key.",
        ],
        "scheduler" => &[
            "Schedule handler panics are handled as schedule failures and route through `ScheduleOptions::on_failure`.",
            "Scheduler hooks are isolated: hook panics are logged and do not crash the scheduler task.",
            "`SchedulerConfig.shutdown_timeout_ms` defaults to `30000`; `0` aborts active schedules immediately on shutdown.",
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
        assert!(config.contains("JobsConfig"));
        assert!(config.contains("HttpConfig"));
        assert!(config.contains("history_retention_days"));
        assert!(config.contains("ObservabilityConfig.enabled"));
        assert!(config.contains("SchedulerConfig"));
        assert!(config.contains("0` aborts"));

        let mut jobs = String::new();
        append_module_notes("jobs", &mut jobs);
        assert!(jobs.contains("JobsConfig.shutdown_timeout_ms"));
        assert!(jobs.contains("lease expiry"));
        assert!(jobs.contains("retry/dead-letter"));
        assert!(jobs.contains("job_history"));
        assert!(jobs.contains("spawn_worker(app)"));
        assert!(jobs.contains("app.background_shutdown_timeout_ms"));

        let mut scheduler = String::new();
        append_module_notes("scheduler", &mut scheduler);
        assert!(scheduler.contains("Schedule handler panics"));
        assert!(scheduler.contains("SchedulerConfig.shutdown_timeout_ms"));

        let mut logging = String::new();
        append_module_notes("logging", &mut logging);
        assert!(logging.contains("/_forge/runtime"));
        assert!(logging.contains("/_forge/metrics"));
        assert!(logging.contains("Prometheus"));
        assert!(logging.contains("capture_enabled"));

        let mut http = String::new();
        append_module_notes("http", &mut http);
        assert!(http.contains("security_headers"));
        assert!(http.contains("trusted_proxy"));
        assert!(http.contains("CSRF"));
        assert!(http.contains("413"));
        assert!(http.contains("actor_or_ip"));
    }
}
