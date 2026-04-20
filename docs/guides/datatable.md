# Datatable

Forge datatables now use a single `Datatable` trait for both model-backed tables and projection/report rows.

## Registering a Datatable

```rust
use forge::prelude::*;

#[async_trait]
impl ServiceProvider for AppServiceProvider {
    async fn register(&self, registrar: &mut ServiceRegistrar) -> Result<()> {
        registrar.register_datatable::<OrdersDatatable>()?;
        registrar.register_datatable::<MerchantSalesDatatable>()?;
        Ok(())
    }
}
```

The same registration path works for app providers and plugins.

## The `Datatable` Trait

```rust
#[async_trait]
trait Datatable: Send + Sync + 'static {
    type Row: Serialize + Send + Sync + 'static;
    type Query: DatatableQuery<Self::Row>;

    const ID: &'static str;

    fn query(ctx: &DatatableContext) -> Self::Query;
    fn columns() -> Vec<DatatableColumn<Self::Row>>;

    fn mappings() -> Vec<DatatableMapping<Self::Row>> { vec![] }
    async fn filters(ctx: &DatatableContext, query: Self::Query) -> Result<Self::Query> { Ok(query) }
    async fn available_filters(ctx: &DatatableContext) -> Result<Vec<DatatableFilterRow>> { Ok(vec![]) }
    fn default_sort() -> Vec<DatatableSort<Self::Row>> { vec![] }

    async fn json(app, actor, request) -> Result<DatatableJsonResponse>;
    async fn download(app, actor, request) -> Result<Response>;
    async fn queue_email(app, actor, request, recipient) -> Result<DatatableExportAccepted>;
}
```

`type Query` is usually one of:

- `ModelQuery<MyModel>`
- `ProjectionQuery<MyProjection>`

## Column DX

`DatatableColumn::field(...)` accepts either:

- a model `Column<M, T>`
- a projection `ProjectionField<P, T>`

Common builders:

```rust
DatatableColumn::field(Order::ID).label("Order").sortable().exportable();
DatatableColumn::field(Order::TOTAL).filterable();
DatatableColumn::field(SalesRow::TOTAL).sortable();
DatatableColumn::field(SalesRow::TOTAL).filter_having(Expr::function("SUM", [Expr::column(Order::TOTAL.column_ref())]));
DatatableColumn::field(SalesRow::MERCHANT_ID).filter_by(SalesRow::MERCHANT_ID.column_ref_from("orders"));
```

Rules:

- model fields get implicit sort/filter targets
- projection fields get implicit sort-by-alias support
- projection auto-filtering is explicit: use `filter_by(...)` for `WHERE` and `filter_having(...)` for `HAVING`

## Model Datatable Example

```rust
use forge::prelude::*;
use serde::Serialize;

#[derive(Debug, Serialize, Model)]
#[forge(model = "orders", primary_key_strategy = "manual")]
struct Order {
    id: i64,
    merchant_id: i64,
    total: i64,
}

struct OrdersDatatable;

#[async_trait]
impl Datatable for OrdersDatatable {
    type Row = Order;
    type Query = ModelQuery<Order>;

    const ID: &'static str = "orders";

    fn query(_ctx: &DatatableContext) -> Self::Query {
        Order::query()
    }

    fn columns() -> Vec<DatatableColumn<Self::Row>> {
        vec![
            DatatableColumn::field(Order::ID).label("Order").sortable().exportable(),
            DatatableColumn::field(Order::MERCHANT_ID)
                .label("Merchant")
                .filterable()
                .exportable(),
            DatatableColumn::field(Order::TOTAL)
                .label("Total")
                .sortable()
                .filterable()
                .exportable(),
        ]
    }

    fn default_sort() -> Vec<DatatableSort<Self::Row>> {
        vec![DatatableSort::desc(Order::ID)]
    }
}
```

## Projection / Report Example

```rust
use forge::prelude::*;
use serde::Serialize;

#[derive(Debug, Serialize, Model)]
#[forge(model = "orders", primary_key_strategy = "manual")]
struct Order {
    id: i64,
    merchant_id: i64,
    total: i64,
}

#[derive(Clone, Debug, Serialize, Projection)]
struct MerchantSalesRow {
    merchant_id: i64,
    order_count: i64,
    total: Option<i64>,
}

struct MerchantSalesDatatable;

#[async_trait]
impl Datatable for MerchantSalesDatatable {
    type Row = MerchantSalesRow;
    type Query = ProjectionQuery<MerchantSalesRow>;

    const ID: &'static str = "merchant-sales";

    fn query(_ctx: &DatatableContext) -> Self::Query {
        MerchantSalesRow::source("orders")
            .select_source(MerchantSalesRow::MERCHANT_ID, "orders")
            .select_aggregate(AggregateProjection::<i64>::count_all(
                MerchantSalesRow::ORDER_COUNT.alias(),
            ))
            .select_aggregate(AggregateProjection::<Option<i64>>::sum(
                Order::TOTAL.column_ref(),
                MerchantSalesRow::TOTAL.alias(),
            ))
            .group_by(MerchantSalesRow::MERCHANT_ID.column_ref_from("orders"))
    }

    fn columns() -> Vec<DatatableColumn<Self::Row>> {
        vec![
            DatatableColumn::field(MerchantSalesRow::MERCHANT_ID)
                .label("Merchant")
                .sortable()
                .filter_by(MerchantSalesRow::MERCHANT_ID.column_ref_from("orders"))
                .exportable(),
            DatatableColumn::field(MerchantSalesRow::ORDER_COUNT)
                .label("Orders")
                .sortable()
                .exportable(),
            DatatableColumn::field(MerchantSalesRow::TOTAL)
                .label("Revenue")
                .sortable()
                .filter_having(Expr::function(
                    "SUM",
                    [Expr::column(Order::TOTAL.column_ref())],
                ))
                .exportable(),
        ]
    }

    fn default_sort() -> Vec<DatatableSort<Self::Row>> {
        vec![DatatableSort::desc(MerchantSalesRow::TOTAL)]
    }
}
```

## Generic Runtime Registry

Every registered datatable is available through the app registry:

```rust
async fn datatable_json(
    State(app): State<AppContext>,
    Path(datatable_id): Path<String>,
    CurrentActor(actor): CurrentActor,
    Query(request): Query<DatatableRequest>,
) -> Result<Json<DatatableJsonResponse>> {
    let registry = app.datatables()?;
    let datatable = registry
        .get(&datatable_id)
        .ok_or_else(|| Error::not_found(format!("datatable '{datatable_id}' not found")))?;

    Ok(Json(
        datatable.json(&app, Some(&actor), request).await?,
    ))
}
```

The same registry-backed object also supports:

- `datatable.download(&app, actor, request).await?`
- `datatable.queue_email(&app, actor, request, "ops@example.com").await?`

## Filter Metadata

Use `available_filters()` when the frontend needs declarative filter controls:

```rust
async fn available_filters(_ctx: &DatatableContext) -> Result<Vec<DatatableFilterRow>> {
    Ok(vec![
        DatatableFilterRow::pair(
            DatatableFilterField::text_search("merchant_query", "Merchant")
                .server_field("merchant_id"),
            DatatableFilterField::decimal_min("minimum_total", "Minimum Total")
                .server_field("total"),
        ),
    ])
}
```

Each filter field now declares:

- `name`: frontend control id
- `binding.field`: server-side filter field
- `binding.op`: backend-declared operator
- `binding.value_kind`: how the frontend should serialize the value

Forge also ships semantic helpers for common cases:

```rust
DatatableFilterField::text_like("email", "Email");
DatatableFilterField::text_search("query", "Search").server_field("email|name");
DatatableFilterField::date_from("created_after", "Created After").server_field("created_at");
DatatableFilterField::date_to("created_before", "Created Before").server_field("created_at");
DatatableFilterField::decimal_min("minimum_amount", "Minimum Amount").server_field("amount");
DatatableFilterField::decimal_max("maximum_amount", "Maximum Amount").server_field("amount");
```

`DatatableFilterField::text(...)` still represents an exact match. Use the semantic helpers above when the UI intends partial-match search or range filters.

Forge still accepts structured `DatatableRequest` filters and legacy `f-...` query params through `DatatableRequest::from_query_params()`, but explicit binding metadata is now the preferred frontend contract.

## Output Modes

Each datatable gets three output modes with identical scoping/filtering:

- `Datatable::json(...)`
- `Datatable::download(...)`
- `Datatable::queue_email(...)`

That keeps model tables and grouped report tables on the same framework path end-to-end.
