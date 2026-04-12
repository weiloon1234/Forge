# Datatable Blueprint Status

The datatable-system blueprint in [rust_datatable_system_blueprint_framework_level.md](../rust_datatable_system_blueprint_framework_level.md) is implemented in Forge.

This status note maps the blueprint's goals to the concrete surfaces already in the repo. It exists so the blueprint can remain a stable design record while contributors can quickly see what implements it today.

## Status

- Blueprint scope: core implementation complete
- First-class target: model-backed datatables with JSON, download, and email export modes
- Pending items: XLSX download requires `rust_xlsxwriter` dependency (stubbed), full job dispatch for email export (stubbed), acceptance tests

## Module Structure

All datatable modules live under `src/datatable/`:

| File | Purpose |
|------|---------|
| `mod.rs` | Module root + re-exports |
| `value.rs` | `DatatableValue` enum (Null, String, Number, Bool, Date, DateTime) |
| `column.rs` | `DatatableColumn<M>` builder with `::field()` constructor |
| `mapping.rs` | `DatatableMapping<M>` for computed output fields |
| `sort.rs` | `DatatableSort<M>` with `::asc()` / `::desc()` constructors |
| `request.rs` | `DatatableRequest`, `DatatableFilterInput`, `DatatableSortInput` |
| `filter_meta.rs` | `DatatableFilterField`, `DatatableFilterRow`, `DatatableFilterOption` |
| `filter_engine.rs` | Auto-filter application + legacy param normalization |
| `context.rs` | `DatatableContext` (scoped execution context) |
| `datatable_trait.rs` | `ModelDatatable` + `ProjectionDatatable` traits |
| `response.rs` | `DatatableJsonResponse`, column/pagination meta |
| `json.rs` | JSON output mode (paginated) |
| `download.rs` | XLSX download mode (stubbed pending `rust_xlsxwriter`) |
| `export.rs` | `DatatableExportDelivery` contract + `NoopExportDelivery` |
| `export_job.rs` | Queued export dispatch (stubbed pending full job wiring) |
| `registry.rs` | `DatatableRegistry` + `DatatableRegistryBuilder` (type-erased lookup by ID) |

## Blueprint Mapping

### Core Types (Blueprint: Columns and Mappings)

- `DatatableColumn<M>` with `::field(column)` constructor capturing column name and db type
- Builder methods: `.sortable()`, `.filterable()`, `.exportable()`, `.label()`, `.relation()`
- `DatatableMapping<M>` with `::new(name, |row, ctx| ...)` for computed/override fields
- `DatatableValue` enum with constructors and `Into<serde_json::Value>` conversion
- `DatatableSort<M>` with typed `::asc(column)` / `::desc(column)` constructors

### Request Shape (Blueprint: Request Shape)

- `DatatableRequest` with page, per_page, sort, filters, search
- Helper methods: `.text()`, `.bool()`, `.date()`, `.datetime()`, `.values()`
- `DatatableFilterInput` with field, op, value
- `DatatableFilterOp` enum covering Eq, Like, date ranges, In, Has, etc.
- `DatatableFilterValue` enum (Text, Bool, Number, Values)

### Filter Metadata (Blueprint: Filter Field Types)

- `DatatableFilterKind`: Text, Select, Checkbox, Date, DateTime
- `DatatableFilterField` with typed constructors: `::text()`, `::select()`, `::checkbox()`, `::date()`, `::datetime()`
- Builder helpers: `.placeholder()`, `.options()`, `.help()`, `.nullable()`
- `DatatableFilterRow::single()` / `::pair()` for layout
- `DatatableFilterOption::new(value, label)` for select options
- Options accept both `Vec` and `Collection` via `Into<Collection<>>`

### Auto-Filter Engine (Blueprint: Filter System)

- Legacy param normalization: `normalize_legacy_params()` supporting f-like-, f-date-, f-gte-, etc.
- `DatatableRequest::from_query_params()` for legacy input
- `apply_auto_filters()` building `Condition` from `ColumnRef` + `DbType`
- `apply_sorts()` with column validation against declared sortable columns
- Supports all filter ops: Eq, Like, Gt/Gte/Lt/Lte, Date/DateFrom/DateTo, DateTime ranges, In, Has, HasLike, LikeAny

### Traits (Blueprint: Core Datatable Shape)

- `ModelDatatable` trait with associated `Model` type, `ID`, `query()`, `columns()`, `mappings()`, `filters()`, `available_filters()`, `default_sort()`
- Provided methods: `json()`, `download()`, `queue_email()` delegating to output modules
- `ProjectionDatatable` trait as escape hatch for grouped/aggregate tables
- `DatatableContext` with `app`, `actor`, `request`, `locale`, `timezone` + `t()` helper

### Output Modes (Blueprint: Output Modes)

- **JSON**: `build_json_response()` in `json.rs` — scoped query, auto-filter, custom filter hook, sorting, pagination, row building with column extraction + mapping overrides
- **Download**: `build_download_response()` in `download.rs` — stubbed, requires `rust_xlsxwriter`
- **Email**: `dispatch_export()` in `export_job.rs` — stubbed, returns `DatatableExportAccepted`

### Export Contract (Blueprint: Export Contract)

- `DatatableExportDelivery` trait with `deliver()` method
- `GeneratedDatatableExport` payload with datatable_id, filename, data bytes, columns
- `NoopExportDelivery` as default/log implementation
- `DatatableActorSnapshot` for serializing actor state into export jobs

### Registry (Blueprint: Registry and Resolution)

- `DatatableRegistry` with `get(id)` and `ids()` for type-erased lookup
- `DatatableRegistryBuilder` with shared-handle pattern (Arc<Mutex<>>)
- `DynDatatable` trait as type-erased interface
- `DatatableAdapter<D>` bridging `ModelDatatable` to `DynDatatable`
- `ServiceRegistrar::register_datatable::<D>()` for provider registration
- `AppContext::datatables()` for runtime resolution

### AST Gap Filled

- `ComparisonOp::Like` and `ComparisonOp::NotLike` added to database AST
- `Column<M, T>::like()` and `.not_like()` methods for typed LIKE queries
- Compiler support for LIKE/NOT LIKE SQL generation

### Framework Integration

- `pub mod datatable` in `src/lib.rs`
- All primary types re-exported from `src/lib.rs` and `src/prelude.rs`
- Datatable registry frozen during bootstrap and registered as singleton
- Accessible via `app.datatables()?` on `AppContext`

## Remaining Work

- [ ] Add `rust_xlsxwriter` dependency and implement full XLSX download
- [ ] Wire full job dispatch in `export_job.rs` for async email exports
- [ ] Acceptance tests for JSON response, filter engine, legacy params, XLSX, registry
- [ ] Relation-based auto-filters (filtering on joined table columns)
