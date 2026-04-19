# Forge API Surface

> Auto-generated from `cargo doc`. Regenerate: `make api-docs`

Each file documents one module's public API (structs, enums, traits, functions).
Load only the file you need — don't read them all at once.

| Module | Description | Size |
|--------|-------------|------|
| [root](root.md) | Crate root: derive macros, re-exports | 16L |
| [app_enum](modules/app_enum.md) | Enum metadata and serialization (ForgeAppEnum) | 26L |
| [attachments](modules/attachments.md) | File attachments with lifecycle (HasAttachments) | 39L |
| [auth](modules/auth.md) | Auth: guards, policies, tokens, sessions, password reset, email verification | 139L |
| [cache](modules/cache.md) | In-memory and Redis-backed caching (CacheManager) | 26L |
| [cli](modules/cli.md) | CLI command registration (CommandRegistry) | 19L |
| [config](modules/config.md) | TOML-based configuration (ConfigRepository, AppConfig, etc.) | 63L |
| [countries](modules/countries.md) | Built-in country data (250 countries) | 27L |
| [database](modules/database.md) | AST-first query system: models, relations, projections, compiler | 676L |
| [datatable](modules/datatable.md) | Server-side datatables: filtering, sorting, pagination, XLSX export | 175L |
| [email](modules/email.md) | Multi-driver email: SMTP, Mailgun, Postmark, Resend, SES | 159L |
| [events](modules/events.md) | Domain event bus with typed listeners | 22L |
| [foundation](modules/foundation.md) | Core: App, AppBuilder, AppContext, AppTransaction, Error, ServiceProvider | 131L |
| [http](modules/http.md) | HTTP: routes, middleware (CORS, CSRF, rate limit, etc.), cookies, resources | 268L |
| [i18n](modules/i18n.md) | Internationalization: locale extraction, translation catalogs | 28L |
| [imaging](modules/imaging.md) | Image processing pipeline (resize, crop, rotate, format conversion) | 36L |
| [jobs](modules/jobs.md) | Background job queue with leased at-least-once delivery | 48L |
| [kernel](modules/kernel.md) | 5 runtime kernels: HTTP, CLI, Scheduler, Worker, WebSocket | 67L |
| [logging](modules/logging.md) | Structured logging, observability, health probes, diagnostics | 70L |
| [metadata](modules/metadata.md) | Key-value metadata for models (HasMetadata) | 21L |
| [notifications](modules/notifications.md) | Multi-channel notifications: email, database, broadcast | 35L |
| [openapi](modules/openapi.md) | OpenAPI 3.1.0 spec generation (ApiSchema, RouteDoc) | 38L |
| [plugin](modules/plugin.md) | Compile-time plugin system with dependency validation | 98L |
| [redis](modules/redis.md) | Namespaced Redis wrapper (RedisManager, RedisConnection) | 40L |
| [scheduler](modules/scheduler.md) | Cron + interval scheduling with Redis-safe leadership | 48L |
| [settings](modules/settings.md) |  | 37L |
| [storage](modules/storage.md) | File storage: local + S3, multipart uploads, file validation | 115L |
| [support](modules/support.md) | Utilities: typed IDs, datetime/clock, Collection<T>, crypto, hashing, locks | 203L |
| [testing](modules/testing.md) | Test infrastructure: TestApp, TestClient, Factory | 37L |
| [translations](modules/translations.md) | Model field translations across locales (HasTranslations) | 26L |
| [typescript](modules/typescript.md) |  | 13L |
| [validation](modules/validation.md) | Validation: 38+ rules, custom rules, request validation extractor | 149L |
| [websocket](modules/websocket.md) | Channel-based WebSocket with presence and typed messages | 61L |

**Total: 33 modules, 2956 lines across all files.**
