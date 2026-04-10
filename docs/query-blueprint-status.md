# Query Blueprint Status

The query-system blueprint in [rust_query_system_blueprint_v_1_v_3.md](../rust_query_system_blueprint_v_1_v_3.md) is complete in Forge.

This status note maps the blueprint's v1-v3 goals to the concrete surfaces, examples, and acceptance coverage already in the repo. It exists so the blueprint can remain a stable design record while contributors can quickly see what implements it today.

## Status

- Blueprint scope: complete
- First-class target: Postgres query system
- Post-blueprint DB work: Rust migration/seeder lifecycle and runtime hardening are implemented separately and are not part of the v1-v3 query blueprint scope

## Blueprint Mapping

### AST-First Foundation

Forge implements the AST-first architecture described in the blueprint through the public database AST and compiler surface:

- `database::ast` with `QueryAst`, `QueryBody`, `Expr`, `Condition`, `JoinNode`, `RelationNode`, and aggregate/window/set-operation nodes
- `PostgresCompiler` and `to_compiled_sql()` on query surfaces
- `Query`, `ModelQuery`, and `ProjectionQuery` as builder layers over AST, not SQL-string builders

References:

- Generic builder example: [examples/phase3_database_generic.rs](../examples/phase3_database_generic.rs)
- Projection/advanced AST example: [examples/phase3_database_projection.rs](../examples/phase3_database_projection.rs)
- Acceptance coverage: [tests/database_acceptance.rs](../tests/database_acceptance.rs) `advanced_projection_queries_support_cte_case_json_union_and_numeric_aggregates`

### Phase 1 — Foundation (v1)

The blueprint's v1 scope is covered by:

- raw SQL execution through `DatabaseManager` and `QueryExecutor`
- parameter binding and transactions
- generic `Query::table(...)` builder
- pagination helpers and compiled-SQL inspection

References:

- Example: [examples/phase3_database_generic.rs](../examples/phase3_database_generic.rs)
- Acceptance coverage: [tests/database_acceptance.rs](../tests/database_acceptance.rs) `typed_runtime_supports_production_postgres_values_and_custom_adapters`

### Phase 2 — Typed Query Builder (v2)

The blueprint's typed model/codegen layer is covered by:

- `Model`, `Column`, `TableMeta`, `ModelQuery`
- `CreateModel`, `CreateManyModel`, `UpdateModel`
- `Projection`, `ProjectionField`, `ProjectionQuery`
- derive-assisted metadata via `forge::Model` and `forge::Projection`
- explicit handwritten relations layered on top of generated metadata

References:

- Typed model example: [examples/phase3_database_model.rs](../examples/phase3_database_model.rs)
- Projection example: [examples/phase3_database_projection.rs](../examples/phase3_database_projection.rs)
- Derive and compile coverage: [tests/derive_ui.rs](../tests/derive_ui.rs)

### Phase 3 — Advanced ORM Layer (v3)

The blueprint's final target behavior is covered by:

- recursive relation-tree eager loading
- `where_has`
- relation aggregates
- `Loaded<T>` hydration
- explicit many-to-many definitions with optional pivot projection hydration

Canonical final-target example:

- [examples/phase3_database_relations.rs](../examples/phase3_database_relations.rs)

Supporting examples:

- Many-to-many and pivot data: [examples/phase3_database_many_to_many.rs](../examples/phase3_database_many_to_many.rs)

Acceptance coverage:

- Relation tree and unlimited-depth eager loading: [tests/database_acceptance.rs](../tests/database_acceptance.rs) `relation_tree_eager_loads_without_hardcoded_depth`
- Many-to-many and relation aggregates: [tests/database_acceptance.rs](../tests/database_acceptance.rs) `many_to_many_relations_load_pivot_data_and_aggregates`
- Typed projections with CTE/UNION/CASE/JSON/aggregates: [tests/database_acceptance.rs](../tests/database_acceptance.rs) `advanced_projection_queries_support_cte_case_json_union_and_numeric_aggregates`

### Codegen vs Handwritten Boundary

Forge matches the blueprint's intended boundary:

- codegen assists metadata, columns, and projections
- writes use model-first fluent builders instead of dedicated payload structs
- relationships remain handwritten and explicit in application code
- business-specific relation semantics are not generated

References:

- Manual relation definitions: [examples/phase3_database_relations.rs](../examples/phase3_database_relations.rs)
- Many-to-many relation definitions: [examples/phase3_database_many_to_many.rs](../examples/phase3_database_many_to_many.rs)
- Proc-macro implementation: [forge-macros](../forge-macros)

### Raw SQL Escape Hatch

Forge keeps raw SQL permanently available through:

- `DatabaseManager::raw_query(...)`
- `DatabaseManager::raw_execute(...)`
- `QueryExecutor` raw execution helpers

References:

- Runtime/value acceptance coverage: [tests/database_acceptance.rs](../tests/database_acceptance.rs) `typed_runtime_supports_production_postgres_values_and_custom_adapters`

## Coverage Notes

The blueprint is intentionally narrower than the full current database platform.

The following are implemented in Forge but are post-blueprint work and should not be treated as unfinished blueprint scope:

- Rust migration and seeding lifecycle
- runtime hardening, native streaming, and statement timeouts
