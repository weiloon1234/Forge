# Rust Backend Framework Blueprint (Kernel + Modular Architecture)

## Overview

This document defines a **framework-level architecture** for a modern Rust backend framework.

The goal:

> Allow application projects to remain thin, focusing only on **bootstrap + registration**, while the framework handles runtime, orchestration, and infrastructure.

---

# Framework Naming (Serious Options)

Choose something that reflects:
- performance
- structure
- control
- extensibility

### Strong Name Candidates

- **Forge** (clean, powerful, production feel)
- **RustForge** (explicit)
- **Corex** (core + engine)
- **Axion** (fast, modern)
- **KernelX** (kernel-focused)
- **FluxCore** (flow + core)
- **IronGate** (backend gateway feel)
- **ArcForge** (architecture + forge)

### Recommendation

> **RustForge** or simply **Forge**

Reason:
- conveys building systems
- aligns with backend + infra mindset
- strong brand positioning

---

# Architectural Style

This framework follows:

**Modular Layered Architecture with Application Kernels**

Influenced by:
- Clean Architecture
- Laravel Kernel / Service Provider pattern
- Hexagonal Architecture (partial)

---

# Core Philosophy

## Project SHOULD NOT:
- manage server lifecycle
- manually wire dependencies everywhere
- duplicate infrastructure logic

## Project SHOULD:
- define domains
- define use cases
- define portals
- register modules into framework

---

# Final Goal (Consumer Experience)

A project should look like:

```rust
use forge::prelude::*;

fn main() -> Result<()> {
    App::builder()
        .load_env()
        .load_config_dir("config")
        .register_provider(AppServiceProvider)
        .register_routes(app::portals::router)
        .register_commands(app::commands::register)
        .register_schedule(app::schedules::register)
        .register_validation_rule("mobile", MobileRule)
        .run_http()?;

    Ok(())
}
```

This is the **target developer experience**.

---

# Framework Structure

```text
src/
├── foundation/
├── kernel/
├── http/
├── websocket/
├── scheduler/
├── cli/
├── validation/
├── auth/
├── events/
├── jobs/
├── config/
├── logging/
├── database/
├── support/
├── prelude.rs
└── lib.rs
```

---

# 1. foundation/

The heart of the framework.

## Responsibilities
- App builder
- Dependency container
- Lifecycle management
- Module/service provider system

## Key Components

### App
- global runtime container

### Builder
- fluent bootstrap API

### Container
- service registry / DI

### ServiceProvider
- module registration pattern

---

# 2. kernel/

Runtime boot layers.

## Types

- HTTP Kernel
- CLI Kernel
- Scheduler Kernel
- WebSocket Kernel

## Responsibilities
- boot runtime
- wire registry into execution

---

# 3. http/

HTTP abstraction layer.

## Responsibilities
- routing
- middleware
- request/response
- guards

---

# 4. websocket/

## Responsibilities
- connection handling
- channel system
- message routing

---

# 5. scheduler/

## Responsibilities
- cron jobs
- interval jobs
- job registry

---

# 6. cli/

Artisan-like system.

## Responsibilities
- command registration
- argument parsing
- execution

---

# 7. validation/

Laravel-style validation engine.

## Features
- built-in rules
- custom rule registration
- chainable API

---

# 8. auth/

## Responsibilities
- actor
- role
- permission
- policy

---

# 9. events/

## Responsibilities
- event dispatch
- listeners

---

# 10. jobs/

## Responsibilities
- background job abstraction
- queue integration (future)

---

# 11. config/

## Responsibilities
- env loading
- config merging

---

# 12. logging/

## Responsibilities
- tracing
- request ID

---

# 13. database/

## Responsibilities
- connection
- transaction helpers

---

# 14. support/

## Responsibilities
- utilities

---

# Project Structure (Consumer)

```text
my-app/
├── bootstrap/
│   ├── app.rs
│   ├── http.rs
│   ├── cli.rs
│   ├── scheduler.rs
│   └── websocket.rs
├── app/
│   ├── domains/
│   ├── use_cases/
│   ├── portals/
│   ├── providers/
│   ├── commands/
│   ├── schedules/
│   └── mod.rs
├── config/
└── main.rs
```

---

# Registration System (Key Feature)

## What can be registered

- routes
- websocket routes
- commands
- cron jobs
- validation rules
- event listeners
- service providers

---

# Example Registration

```rust
app.register_routes(router);
app.register_command(MyCommand);
app.register_schedule(schedule_fn);
app.register_validation_rule("phone", PhoneRule);
```

---

# Key Design Principles

1. Thin application layer
2. Strong framework kernel
3. Clear separation of concerns
4. Extensible via providers
5. Registry-driven system

---

# Long-Term Evolution

## Phase 1
- HTTP
- Validation
- CLI
- Scheduler

## Phase 2
- WebSocket
- Events
- Jobs

## Phase 3
- Plugin system
- distributed job system
- observability tools

---

# Final Summary

This framework aims to:

- centralize infrastructure
- standardize backend patterns
- reduce boilerplate
- enforce clean architecture

---

# Final Statement

> Build once in the framework, reuse everywhere in projects.

> Project = configuration + registration
> Framework = execution + orchestration

