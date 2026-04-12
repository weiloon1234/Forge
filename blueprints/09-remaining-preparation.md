# Forge Framework Remaining Preparation Blueprint

> **Status:** Reviewed 2026-04-11. Most items are now done. See status per section.
>
> Defines what still belongs to the **framework stage** before building or stabilizing consumer app scaffolds.

---

# Preparation Status Summary (2026-04-11)

| # | Area | Status | Notes |
|---|------|--------|-------|
| 1 | App Builder Final Contract | ✅ Done | Covered by lifecycle blueprint |
| 2 | Service Provider Lifecycle | ✅ Done | Covered by lifecycle blueprint |
| 3 | Container / Resolution Rules | ✅ Done | Covered by lifecycle blueprint |
| 4 | Runtime Kernel Contracts | ✅ Done | Covered by lifecycle blueprint |
| 5 | Error Model | ✅ Done | Structured `Error` enum with conversions |
| 6 | Database / ORM Core AST | ✅ Done | Full AST query builder, Model trait, relations, migrations |
| 7 | Auth Contract | 🔄 Partial | Actor/role/permission/policy done, custom authenticator TODO |
| 8 | Plugin System Contract | ✅ Done | Lifecycle, dependencies, assets, scaffolding |
| 10 | Crate Split / Workspace | Deferred | Single crate works, split when publish-ready |

---

# Answer First

Yes:

- **Forge Application Entry Points Blueprint** is mainly a **project consumer / app scaffold blueprint**
- it describes how a consumer app should organize its entry files around Forge
- it is **not** the internal framework crate structure itself

So if you want to start a separate codebase for a future consumer app scaffold/template, that is reasonable.

---

# Current Stage Clarification

You are still in the **framework preparation stage**.

That means the most important work now is not the final consumer app layout.

The most important work now is to finish the framework-side contracts that consumer apps will rely on.

---

# What Still Needs Framework-Level Preparation

These are the major remaining blueprint areas that should be settled at framework level.

---

# 1. App Builder Final Contract

> **Status: ✅ Done** — All builder methods implemented. See lifecycle blueprint for details.

## Why this matters

Everything in consumer apps depends on this API being stable:

```rust
App::builder()
```

If this contract keeps drifting, all consumer scaffolds will drift too.

## Must define

- builder method naming
- registration order rules
- config loading order
- runtime mode methods
- middleware registration API
- plugin registration API
- boot hooks
- graceful shutdown hooks

## Example target surface

```rust
App::builder()
    .load_env()
    .load_config_dir("config")
    .register_provider(AppServiceProvider)
    .register_routes(app::portals::router)
    .register_commands(app::commands::register)
    .register_schedule(app::schedules::register)
    .register_validation_rule("mobile", MobileRule)
    .register_middleware(...)
    .run_http()?;
```

---

# 2. Service Provider Lifecycle Blueprint

> **Status: ✅ Done** — Two-phase lifecycle (`register` + `boot`) implemented. See lifecycle blueprint for details.

## Why this matters

You already use provider registration heavily.

Before consumer scaffolds are finalized, you should lock down:

- `register()` vs `boot()` semantics
- when providers may resolve services
- provider ordering
- provider dependency handling
- plugin/provider interaction

## Must define

- provider trait
- provider lifecycle states
- container availability per phase
- idempotency rules
- error handling rules

---

# 3. Container / Dependency Resolution Rules

> **Status: ✅ Done** — Singleton + factory bindings, strict no-overwrite, TypeId-based resolution. See lifecycle blueprint for details.

## Why this matters

Your whole framework depends on container stability.

You should define:

- singleton vs transient
- lazy vs eager resolution
- scoped/request services
- transaction-scoped services if applicable
- override precedence
- test-time replacement/mocking

Without this, framework behavior becomes inconsistent.

---

# 4. Runtime Kernel Contracts

> **Status: ✅ Done** — All 5 kernels (HTTP, CLI, Worker, Scheduler, WebSocket) have clear contracts and receive fully built `BootArtifacts`.

## Why this matters

HTTP / CLI / Worker / WebSocket / Scheduler already exist.

What still needs to be defined cleanly is the contract for each runtime kernel.

## Must define

### HTTP kernel
- middleware order
- route registration timing
- auth injection timing
- request context creation

### CLI kernel
- app boot before command execution
- command context
- no-interaction mode

### Worker kernel
- queue lifecycle
- job deserialization contract
- retry/dead-letter rules
- shutdown behavior

### Scheduler kernel
- lease acquisition flow
- overlap prevention
- schedule registration contract

### WebSocket kernel
- connection lifecycle
- auth timing
- channel/room behavior
- distributed pub/sub guarantees

---

# 5. Error Model Blueprint

> **Status: ✅ Done** — `Error` enum with `Message`, `Http`, `NotFound`, `Other`. Consistent JSON responses. `From<ValidationErrors>` and `From<AuthError>` conversions.

## Why this matters

You already have a structured `Error` enum.

Now define the framework-wide error contract fully.

## Must define

- canonical error categories
- HTTP rendering rules
- CLI rendering rules
- worker failure rules
- validation/auth/database conversion rules
- logging behavior for each error class

This should become a permanent framework law.

---

# 6. Database / ORM Core AST Blueprint

> **Status: ✅ Done** — Full AST-based query builder, `Model` trait + `#[derive(Model)]`, relations (has_many, has_one, belongs_to, many_to_many), eager loading, migrations, seeders. Massively exceeds original scope.

## Why this matters

This is one of the biggest unresolved core foundations.

You already decided AST-first is critical.

Before more app scaffolds are finalized, framework should define:

- query AST
- expression tree
- condition tree
- relation node tree
- SQL compiler boundaries
- eager loading planner contract

This is still framework-level, not app-level.

---

# 7. Auth Contract Blueprint

> **Status: 🔄 Partially done**
>
> **Done:** Actor, AuthError, BearerAuthenticator trait, Policy trait, AuthManager, Authorizer, StaticBearerAuthenticator, CurrentActor/OptionalActor extractors, route-level auth via AccessScope.
>
> **TODO:** Custom authenticator — user will implement own auth method (not JWT). Database-backed authenticator is the main remaining gap.

## Why this matters

Auth is still partial.

Before consumer scaffolds are considered stable, framework should define:

- authenticator contract
- actor model
- guard model
- permission model
- policy model
- optional actor extractor behavior
- route option integration

Especially because you do **not** want JWT assumptions forced into the framework.

---

# 8. Plugin System Contract Blueprint

> **Status: ✅ Done** — Plugin lifecycle, dependency resolution, assets, scaffolding, CLI commands all implemented.

## Why this matters

Plugin/module support is already present.

This needs a stable contract before consumer apps rely heavily on it.

## Must define

- plugin lifecycle
- plugin dependency graph rules
- plugin assets behavior
- plugin migrations/seeds/commands integration
- plugin route registration timing
- plugin config override rules

---

# 9. Framework Crate Split / Workspace Blueprint

## Why this matters

This is important for compile time, modularity, and publish strategy.

You should define whether Forge remains one crate or is split into:

- forge-core
- forge-http
- forge-validation
- forge-i18n
- forge-logging
- forge-db
- forge-cli
- forge-jobs
- forge-websocket
- forge-storage
- forge-email

This is a framework architecture issue, not a consumer app issue.

---

# Priority Recommendation (Updated 2026-04-11)

## ✅ Done — No further work needed
1. App Builder Final Contract
2. Service Provider Lifecycle
3. Container / Resolution Rules
4. Database AST / ORM Core
5. Runtime Kernel Contracts
6. Error Model Blueprint
7. Plugin Contract Blueprint

## 🔄 Remaining
8. **Auth Contract** — Custom/database-backed authenticator (user will implement own)
9. **Crate Split / Workspace** — Deferred until publish-ready

---

# What This Means For Your Codebases

## Current framework codebase

Keep using it for:
- Forge internals
- builder API
- kernels
- providers
- container
- ORM core
- auth contracts
- plugin contracts

## Optional new consumer scaffold codebase

Only create this if you want to separately design:
- starter app layout
- generated project template
- example app using Forge
- recommended entry points
- example domain/use_case/portal modules

That would be a **consumer scaffold / starter template repo**, not the main framework repo.

---

# Final Decision Guidance

## If your goal is framework stabilization
Do **not** switch focus fully to consumer scaffold yet.

You still have several framework-level contracts that should be locked first.

## If your goal is template experimentation
Then yes, create a separate scaffold codebase and keep it loosely coupled for now.

---

# Strong Recommendation

The framework preparation stage is **largely complete**. 8 out of 9 original items are done.

The remaining item (Auth custom authenticator) is dependent on the user's specific project requirements — the framework provides the trait and infrastructure (`BearerAuthenticator`, `AuthManager`, `CurrentActor` extractor). The user will implement their own authentication method.

---

# Final Statement

> Entry-point blueprint = mostly consumer app scaffold territory.
>
> Framework stage is not finished yet.
>
> Lock the core contracts first, then freeze the scaffold shape.

