# Forge

Forge is a strongly typed Rust backend framework built around thin application bootstraps and a strong framework kernel.

The core idea is simple:

- Project code focuses on bootstrap, registration, and domain behavior.
- Forge owns runtime boot, orchestration, infrastructure wiring, and cross-cutting concerns.

## Quick Start

```rust
use async_trait::async_trait;
use forge::prelude::*;

const MOBILE_RULE: ValidationRuleId = ValidationRuleId::new("mobile");

#[derive(Clone)]
struct AppServiceProvider;

struct MobileRule;

#[async_trait]
impl ServiceProvider for AppServiceProvider {}

#[async_trait]
impl ValidationRule for MobileRule {
    async fn validate(
        &self,
        _context: &RuleContext,
        value: &str,
    ) -> std::result::Result<(), ValidationError> {
        if value.starts_with('+') {
            Ok(())
        } else {
            Err(ValidationError::new("mobile", "invalid mobile"))
        }
    }
}

fn main() -> Result<()> {
    App::builder()
        .load_env()
        .load_config_dir("config")
        .register_provider(AppServiceProvider)
        .register_routes(app::portals::router)
        .register_commands(app::commands::register)
        .register_schedule(app::schedules::register)
        .register_validation_rule(MOBILE_RULE, MobileRule)
        .run_http()?;

    Ok(())
}

mod app {
    pub mod portals {
        use forge::prelude::*;

        pub fn router(registrar: &mut HttpRegistrar) -> Result<()> {
            registrar.route("/health", get(health));
            Ok(())
        }

        async fn health() -> impl IntoResponse {
            StatusCode::OK
        }
    }

    pub mod commands {
        use forge::prelude::*;

        const PING_COMMAND: CommandId = CommandId::new("ping");

        pub fn register(registry: &mut CommandRegistry) -> Result<()> {
            registry.command(
                PING_COMMAND,
                Command::new("ping"),
                |_invocation| async move { Ok(()) },
            )?;
            Ok(())
        }
    }

    pub mod schedules {
        use forge::prelude::*;

        const HEARTBEAT_SCHEDULE: ScheduleId = ScheduleId::new("heartbeat");

        pub fn register(registry: &mut ScheduleRegistry) -> Result<()> {
            registry.cron(
                HEARTBEAT_SCHEDULE,
                CronExpression::parse("*/5 * * * * *")?,
                |_invocation| async move { Ok(()) },
            )?;
            Ok(())
        }
    }
}
```

## Primary Bootstraps

- `run_http`: Axum-powered HTTP kernel with validation, auth guards, request IDs, and observability hooks.
- `run_cli`: typed command kernel built on `clap`.
- `run_scheduler`: cron and interval scheduler with leader-safe Redis coordination when configured.
- `run_worker`: background job worker with leased at-least-once delivery.

Advanced but first-class bootstraps:

- `run_websocket`: typed channel-based websocket kernel.
- `register_plugin`: compile-time plugin registration with dependency ordering, config defaults, assets, and scaffolds.

## Framework Status

- Phase 1: HTTP, validation, CLI, scheduler are implemented.
- Phase 2: websocket, events, and jobs are implemented.
- Auth + typing realignment: bearer auth, guards, policies, typed semantic IDs, and request identity are implemented.
- Observability: readiness, liveness, runtime diagnostics, and typed runtime counters are implemented.
- Distributed runtime: leased worker processing and cluster-safe scheduler leadership are implemented.
- Plugins: compile-time plugin registry, dependency validation, config defaults, package assets, and scaffolds are implemented.
- Database query blueprint (`v1 -> v3`): complete. Forge ships the AST-first Postgres query system described in [rust_query_system_blueprint_v_1_v_3.md](rust_query_system_blueprint_v_1_v_3.md), including generic builders, typed model/projection queries, model-first `create()/update()/query()` APIs, always-on model lifecycle hooks/events on app-backed writes, explicit handwritten relations, recursive eager loading, `where_has`, aggregates, many-to-many, codegen-assisted metadata, raw SQL escape hatches, and batched `ModelQuery::stream()` support for eager-loaded relations and relation aggregates.
- Database post-blueprint work: Rust migration/seeder lifecycle with build-time discovery and runtime hardening are implemented separately from the query blueprint scope. See [docs/query-blueprint-status.md](docs/query-blueprint-status.md).

## Examples

- [examples/blueprint_http.rs](examples/blueprint_http.rs)
- [examples/blueprint_typed.rs](examples/blueprint_typed.rs)
- [examples/phase2_websocket.rs](examples/phase2_websocket.rs)
- [examples/phase25_auth.rs](examples/phase25_auth.rs)
- [examples/phase3_observability.rs](examples/phase3_observability.rs)
- [examples/phase3_worker.rs](examples/phase3_worker.rs)
- [examples/phase3_database_generic.rs](examples/phase3_database_generic.rs): generic `Query` builder and AST-first foundation
- [examples/phase3_database_model.rs](examples/phase3_database_model.rs): typed `ModelQuery` with model-first `create()/update()` builders and model-local lifecycle hooks
- [examples/phase3_database_relations.rs](examples/phase3_database_relations.rs): canonical v3 relation-tree example with nested `.with(...)`, `where_has`, and relation aggregates
- [examples/phase3_database_projection.rs](examples/phase3_database_projection.rs): typed projections with CTE, `UNION`, `CASE`, and JSON expressions
- [examples/phase3_database_many_to_many.rs](examples/phase3_database_many_to_many.rs): explicit many-to-many relations with pivot projection hydration
- [examples/phase4_database_lifecycle.rs](examples/phase4_database_lifecycle.rs): build-time discovered Rust migrations and seeders with a single provider hook
- [examples/phase3_plugin.rs](examples/phase3_plugin.rs)

## Local Verification

```bash
make verify
make verify-release
FORGE_TEST_POSTGRES_URL=postgres://postgres:postgres@127.0.0.1:5432/forge make test-postgres
```

Those commands cover formatting, tests, clippy, fixture checks, package dry-run verification, and the Postgres-backed database acceptance path.

## Contributing and Releases

- Contributor workflow: [CONTRIBUTING.md](CONTRIBUTING.md)
- Changelog: [CHANGELOG.md](CHANGELOG.md)
- Release checklist: [docs/release-checklist.md](docs/release-checklist.md)
- Query blueprint status: [docs/query-blueprint-status.md](docs/query-blueprint-status.md)
