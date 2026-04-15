# datatable

Server-side datatables: filtering, sorting, pagination, XLSX export

[Back to index](../index.md)

## forge::datatable::column

```rust
struct DatatableColumn
  fn field<T>(column: Column<M, T>) -> Self
  fn label(self, label: impl Into<String>) -> Self
  fn sortable(self) -> Self
  fn filterable(self) -> Self
  fn exportable(self) -> Self
  fn relation(self, relation: impl Into<String>) -> Self
  fn db_type(&self) -> DbType
```

## forge::datatable::context

```rust
struct DatatableContext
  fn new( app: &'a AppContext, actor: Option<&'a Actor>, request: &'a DatatableRequest, ) -> Self
  fn t(&self, key: &str) -> String
```

## forge::datatable::datatable_trait

```rust
trait ModelDatatable
  fn query(ctx: &DatatableContext<'_>) -> ModelQuery<Self::Model>
  fn columns() -> Vec<DatatableColumn<Self::Model>>
  fn mappings() -> Vec<DatatableMapping<Self::Model>>
  fn filters<'life0, 'async_trait>(
  fn available_filters<'life0, 'async_trait>(
  fn default_sort() -> Vec<DatatableSort<Self::Model>>
  fn json<'life0, 'life1, 'async_trait>(
  fn download<'life0, 'life1, 'async_trait>(
  fn queue_email<'life0, 'life1, 'life2, 'async_trait>(
trait ProjectionDatatable
  fn query(ctx: &DatatableContext<'_>) -> ProjectionQuery<Self::Row>
  fn columns() -> Vec<DatatableColumn<Self::Row>>
  fn mappings() -> Vec<DatatableMapping<Self::Row>>
```

## forge::datatable::download

```rust
fn async fn build_download_response<M, D>( app: &AppContext, actor: Option<&Actor>, request: DatatableRequest, ) -> Result<Response>where M: Model + Serialize, D: ModelDatatable<Model = M> + ?Sized,
```

## forge::datatable::export

```rust
struct GeneratedDatatableExport
struct NoopExportDelivery
trait DatatableExportDelivery
  fn deliver<'life0, 'life1, 'async_trait>(
```

## forge::datatable::export_job

```rust
struct DatatableExportJob
struct DatatableExportJobPayload
fn async fn dispatch_export<D: ModelDatatable + ?Sized>( app: &AppContext, actor: Option<&Actor>, request: DatatableRequest, recipient: &str, ) -> Result<DatatableExportAccepted>
```

## forge::datatable::filter_engine

```rust
fn apply_auto_filters<M: Model>( query: ModelQuery<M>, filters: &[DatatableFilterInput], columns: &[DatatableColumn<M>], table_name: &str, ) -> Result<ModelQuery<M>>
fn apply_default_sorts<M: Model>( query: ModelQuery<M>, sorts: &[DatatableSort<M>], table_name: &str, ) -> Result<ModelQuery<M>>
fn apply_sorts<M: Model>( query: ModelQuery<M>, sorts: &[DatatableSortInput], columns: &[DatatableColumn<M>], table_name: &str, ) -> Result<ModelQuery<M>>
```

## forge::datatable::filter_meta

```rust
enum DatatableFilterKind { Text, Select, Checkbox, Date, DateTime }
struct DatatableFilterField
  fn text(name: impl Into<String>, label: impl Into<String>) -> Self
  fn select(name: impl Into<String>, label: impl Into<String>) -> Self
  fn checkbox(name: impl Into<String>, label: impl Into<String>) -> Self
  fn date(name: impl Into<String>, label: impl Into<String>) -> Self
  fn datetime(name: impl Into<String>, label: impl Into<String>) -> Self
  fn placeholder(self, placeholder: impl Into<String>) -> Self
  fn options<I>(self, options: I) -> Self
  fn help(self, help: impl Into<String>) -> Self
  fn nullable(self) -> Self
  fn enum_select<E: ForgeAppEnum>( name: impl Into<String>, label: impl Into<String>, ) -> Self
struct DatatableFilterOption
  fn new(value: impl Into<String>, label: impl Into<String>) -> Self
struct DatatableFilterRow
  fn single(field: DatatableFilterField) -> Self
  fn pair(left: DatatableFilterField, right: DatatableFilterField) -> Self
```

## forge::datatable::json

```rust
fn async fn build_json_response<M, D>( app: &AppContext, actor: Option<&Actor>, request: DatatableRequest, ) -> Result<DatatableJsonResponse>where M: Model + Serialize, D: ModelDatatable<Model = M> + ?Sized,
```

## forge::datatable::mapping

```rust
struct DatatableMapping
  fn new<F>(name: impl Into<String>, callback: F) -> Self
  fn compute(&self, model: &M, ctx: &DatatableContext<'_>) -> DatatableValue
```

## forge::datatable::registry

```rust
struct DatatableAdapter
  fn new() -> Self
struct DatatableRegistry
  fn get(&self, id: &str) -> Option<Arc<dyn DynDatatable>>
  fn ids(&self) -> Vec<&str>
trait DynDatatable
  fn id(&self) -> &str
  fn json<'life0, 'life1, 'life2, 'async_trait>(
  fn download<'life0, 'life1, 'life2, 'async_trait>(
  fn queue_email<'life0, 'life1, 'life2, 'life3, 'async_trait>(
```

## forge::datatable::request

```rust
enum DatatableFilterOp { Show 17 variants    Eq, NotEq, Like, Gt, Gte, ... +12 more }
enum DatatableFilterValue { Text, Bool, Number, Values }
struct DatatableFilterInput
struct DatatableRequest
  fn text(&self, name: &str) -> Option<&str>
  fn bool(&self, name: &str) -> Option<bool>
  fn date(&self, name: &str) -> Option<Date>
  fn datetime(&self, name: &str) -> Option<DateTime>
  fn values(&self, name: &str) -> Collection<String>
  fn from_query_params(params: &HashMap<String, String>) -> Self
struct DatatableSortInput
```

## forge::datatable::response

```rust
struct DatatableActorSnapshot
struct DatatableColumnMeta
struct DatatableExportAccepted
struct DatatableJsonResponse
struct DatatablePaginationMeta
  fn new(page: u64, per_page: u64, total: u64) -> Self
```

## forge::datatable::sort

```rust
struct DatatableSort
  fn asc<T>(column: Column<M, T>) -> Self
  fn desc<T>(column: Column<M, T>) -> Self
```

## forge::datatable::value

```rust
enum DatatableValue { Null, String, Number, Bool, Date, DateTime }
  fn null() -> Self
  fn string(value: impl Into<String>) -> Self
  fn number(value: impl Into<Number>) -> Self
  fn bool(value: bool) -> Self
  fn date(value: Date) -> Self
  fn datetime(value: DateTime) -> Self
```

