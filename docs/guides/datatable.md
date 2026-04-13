# Datatable System Usage Guide

Forge's datatable system provides model-backed, server-side data tables with filtering, sorting, pagination, XLSX export, and async email delivery — all from a single trait implementation.

---

## Quick Start

```rust
use forge::prelude::*;

struct InvoiceDatatable;

impl ModelDatatable for InvoiceDatatable {
    type Model = Invoice;
    const ID: &'static str = "invoices";

    fn query(ctx: &DatatableContext) -> ModelQuery<Invoice> {
        Invoice::model_query()
    }

    fn columns() -> Vec<DatatableColumn<Invoice>> {
        vec![
            DatatableColumn::field(Invoice::ID).label("ID").sortable().exportable(),
            DatatableColumn::field(Invoice::CUSTOMER_NAME).label("Customer").sortable().filterable().exportable(),
            DatatableColumn::field(Invoice::AMOUNT).label("Amount").sortable().filterable().exportable(),
            DatatableColumn::field(Invoice::STATUS).label("Status").filterable().exportable(),
            DatatableColumn::field(Invoice::CREATED_AT).label("Created").sortable().exportable(),
        ]
    }
}
```

Register during bootstrap:

```rust
// In a ServiceProvider:
registrar.register_datatable::<InvoiceDatatable>()?;

// Or in a Plugin:
registrar.register_datatable::<InvoiceDatatable>();
```

Use from an HTTP handler:

```rust
async fn list_invoices(
    State(app): State<AppContext>,
    CurrentActor(actor): CurrentActor,
    Query(request): Query<DatatableRequest>,
) -> impl IntoResponse {
    let response = InvoiceDatatable::json(&app, Some(&actor), request).await?;
    Json(response)
}
```

That's it — you get paginated JSON with column metadata, filter definitions, and pagination info.

---

## Core Concepts

### The ModelDatatable Trait

Every datatable implements `ModelDatatable`. Only two methods are required — `query()` and `columns()`. Everything else has sensible defaults.

```rust
trait ModelDatatable: Send + Sync + 'static {
    type Model: Model + Serialize;
    const ID: &'static str;

    // Required
    fn query(ctx: &DatatableContext) -> ModelQuery<Self::Model>;
    fn columns() -> Vec<DatatableColumn<Self::Model>>;

    // Optional — all have defaults
    fn mappings() -> Vec<DatatableMapping<Self::Model>> { vec![] }
    fn default_sort() -> Vec<DatatableSort<Self::Model>> { vec![] }
    async fn filters(ctx: &DatatableContext, query: ModelQuery<Self::Model>) -> Result<ModelQuery<Self::Model>> { Ok(query) }
    async fn available_filters(ctx: &DatatableContext) -> Result<Vec<DatatableFilterRow>> { Ok(vec![]) }

    // Provided — call from handlers
    async fn json(app, actor, request) -> Result<DatatableJsonResponse>;
    async fn download(app, actor, request) -> Result<Response>;
    async fn queue_email(app, actor, request, recipient) -> Result<DatatableExportAccepted>;
}
```

### DatatableContext

Every datatable method receives a `DatatableContext` that provides the execution scope:

```rust
pub struct DatatableContext<'a> {
    pub app: &'a AppContext,           // Full framework access
    pub actor: Option<&'a Actor>,      // Authenticated user (if any)
    pub request: &'a DatatableRequest, // Pagination, filters, sorts
    pub locale: Option<&'a str>,       // For i18n
    pub timezone: Timezone,            // For date formatting
}
```

Use `ctx.actor` for scoping queries to the current user's data. Use `ctx.t("key")` for translations.

---

## Columns

Columns define what data appears in the table, how it behaves, and what gets exported.

```rust
fn columns() -> Vec<DatatableColumn<Invoice>> {
    vec![
        // Basic column — just displays the value
        DatatableColumn::field(Invoice::ID),

        // Full builder chain
        DatatableColumn::field(Invoice::AMOUNT)
            .label("Total Amount")    // Display name (default: column name)
            .sortable()               // Frontend can sort by this
            .filterable()             // Frontend can filter by this
            .exportable()             // Include in XLSX downloads
            .relation("currencies"),  // Document foreign key (metadata only)
    ]
}
```

**Rules:**
- `.sortable()` — column appears in the sortable column list; frontend can sort ASC/DESC
- `.filterable()` — column participates in auto-filtering; filter operations validated against it
- `.exportable()` — column included in XLSX export; non-exportable columns are skipped
- `.label()` — used as column header in JSON response and XLSX file
- `.relation()` — metadata hint for frontend (not used by filter engine yet)

---

## Computed Columns (Mappings)

Mappings add virtual columns or override existing column values. They receive the model instance and context, and return a `DatatableValue`.

```rust
fn mappings() -> Vec<DatatableMapping<Invoice>> {
    vec![
        // Add a new computed column
        DatatableMapping::new("formatted_amount", |invoice, _ctx| {
            DatatableValue::string(format!("${:.2}", invoice.amount))
        }),

        // Override an existing column's display value
        DatatableMapping::new("status", |invoice, ctx| {
            DatatableValue::string(ctx.t(&format!("invoice.status.{}", invoice.status)))
        }),

        // Boolean computed column
        DatatableMapping::new("is_overdue", |invoice, _ctx| {
            let now = DateTime::now();
            DatatableValue::bool(invoice.due_date < now && invoice.status != "paid")
        }),
    ]
}
```

**DatatableValue types:**

```rust
DatatableValue::null()
DatatableValue::string("text")
DatatableValue::number(42)     // accepts any Into<serde_json::Number>
DatatableValue::bool(true)
DatatableValue::date(date)     // renders as ISO 8601
DatatableValue::datetime(dt)   // renders as ISO 8601
```

**Behavior:**
- A mapping with the same name as a column **overrides** it in the output
- A mapping with a new name **adds** a new field to each row
- Mappings are NOT sortable or filterable (they're computed after query)
- Mappings ARE included in XLSX export

---

## Default Sort

Applied when the request has no explicit sort:

```rust
fn default_sort() -> Vec<DatatableSort<Invoice>> {
    vec![
        DatatableSort::desc(Invoice::CREATED_AT),  // newest first
        DatatableSort::asc(Invoice::ID),            // tie-break by ID
    ]
}
```

If both `default_sort()` returns empty AND the request has no sort, rows are returned in database natural order.

---

## Scoped Queries

The `query()` method defines the base query. Use `ctx.actor` to scope data:

```rust
fn query(ctx: &DatatableContext) -> ModelQuery<Invoice> {
    let mut q = Invoice::model_query();

    // Scope to current user's organization
    if let Some(actor) = ctx.actor {
        q = q.where_col(Invoice::ORG_ID, &actor.claims["org_id"]);
    }

    // Apply search hint if provided
    if let Some(search) = &ctx.request.search {
        q = q.where_col(Invoice::CUSTOMER_NAME, ComparisonOp::Like, format!("%{search}%"));
    }

    q
}
```

This base query is then further refined by auto-filters and custom filters.

---

## Filters

Filters come in two parts: **available filters** (what the frontend shows) and **custom filter logic** (server-side refinement).

### Available Filters (Frontend UI)

Define what filter controls the frontend should render:

```rust
async fn available_filters(ctx: &DatatableContext) -> Result<Vec<DatatableFilterRow>> {
    Ok(vec![
        // Two filters side by side
        DatatableFilterRow::pair(
            DatatableFilterField::text("customer_name", "Customer")
                .placeholder("Search by name..."),
            DatatableFilterField::select("status", "Status")
                .options(vec![
                    DatatableFilterOption::new("pending", "Pending"),
                    DatatableFilterOption::new("paid", "Paid"),
                    DatatableFilterOption::new("overdue", "Overdue"),
                ]),
        ),

        // Single date filter
        DatatableFilterRow::single(
            DatatableFilterField::date("created_at", "Created Date"),
        ),

        // Checkbox + datetime range
        DatatableFilterRow::pair(
            DatatableFilterField::checkbox("is_flagged", "Flagged Only"),
            DatatableFilterField::datetime("updated_at", "Last Updated"),
        ),
    ])
}
```

**Filter kinds:**

| Kind | Frontend control | Auto-filter behavior |
|------|-----------------|---------------------|
| `Text` | Text input | `LIKE %value%` |
| `Select` | Dropdown with options | `= value` |
| `Checkbox` | Toggle | `= true/false` |
| `Date` | Date picker | Supports `Date`, `DateFrom`, `DateTo` ops |
| `DateTime` | Date+time picker | Supports `Datetime`, `DatetimeFrom`, `DatetimeTo` ops |

**Dynamic options** — filters can load options from the database:

```rust
async fn available_filters(ctx: &DatatableContext) -> Result<Vec<DatatableFilterRow>> {
    let db = ctx.app.database()?;
    let statuses = InvoiceStatus::model_query().all(&*db).await?;

    Ok(vec![
        DatatableFilterRow::single(
            DatatableFilterField::select("status", "Status")
                .options(statuses.iter().map(|s| {
                    DatatableFilterOption::new(&s.key, &s.label)
                }).collect::<Vec<_>>()),
        ),
    ])
}
```

### Custom Filter Logic

For filters that can't be expressed as simple column comparisons:

```rust
async fn filters(
    ctx: &DatatableContext,
    mut query: ModelQuery<Invoice>,
) -> Result<ModelQuery<Invoice>> {
    // Custom: filter by amount range from request params
    if let Some(min) = ctx.request.text("min_amount") {
        if let Ok(min) = min.parse::<f64>() {
            query = query.where_col(Invoice::AMOUNT, ComparisonOp::Gte, min);
        }
    }

    // Custom: filter by related customer name (join)
    if let Some(name) = ctx.request.text("customer_name") {
        query = query.where_has(Invoice::customer_relation(), |cq| {
            cq.where_col(Customer::NAME, ComparisonOp::Like, format!("%{name}%"))
        });
    }

    Ok(query)
}
```

### Auto-Filter Operations

When the frontend sends filters, the engine applies them automatically to filterable columns:

| Operation | SQL | Example request |
|-----------|-----|-----------------|
| `Eq` | `WHERE col = $1` | `{"field":"status","op":"Eq","value":{"Text":"paid"}}` |
| `NotEq` | `WHERE col != $1` | |
| `Like` | `WHERE col LIKE '%$1%'` | |
| `Gt` / `Gte` / `Lt` / `Lte` | Numeric comparison | |
| `In` | `WHERE col IN ($1, $2, ...)` | `{"field":"status","op":"In","value":{"Values":["paid","pending"]}}` |
| `DateFrom` | `WHERE col >= $1` | Date range start |
| `DateTo` | `WHERE col <= $1` | Date range end |
| `Has` | `WHERE col IS NOT NULL` | |

---

## Three Output Modes

Every datatable supports three output modes with identical scoping and filtering:

### 1. JSON (paginated)

```rust
async fn list_invoices(
    State(app): State<AppContext>,
    CurrentActor(actor): CurrentActor,
    Query(request): Query<DatatableRequest>,
) -> impl IntoResponse {
    Json(InvoiceDatatable::json(&app, Some(&actor), request).await?)
}
```

Response shape:

```json
{
  "rows": [
    { "id": 1, "customer_name": "Alice", "amount": 150.00, "formatted_amount": "$150.00", "status": "paid" },
    { "id": 2, "customer_name": "Bob", "amount": 75.50, "formatted_amount": "$75.50", "status": "pending" }
  ],
  "columns": [
    { "name": "id", "label": "ID", "sortable": true, "filterable": false },
    { "name": "customer_name", "label": "Customer", "sortable": true, "filterable": true },
    { "name": "amount", "label": "Amount", "sortable": true, "filterable": true }
  ],
  "filters": [
    { "fields": [{ "name": "status", "kind": "Select", "label": "Status", "options": [...] }] }
  ],
  "pagination": { "page": 1, "per_page": 20, "total": 156, "total_pages": 8 },
  "applied_filters": [...],
  "sorts": [{ "field": "created_at", "direction": "Desc" }]
}
```

### 2. XLSX Download (full dataset)

```rust
async fn download_invoices(
    State(app): State<AppContext>,
    CurrentActor(actor): CurrentActor,
    Query(request): Query<DatatableRequest>,
) -> impl IntoResponse {
    InvoiceDatatable::download(&app, Some(&actor), request).await
}
```

Returns an XLSX file with:
- Headers from column `.label()` values (bold)
- Only `.exportable()` columns included
- Mapping overrides applied
- Type-aware cells (numbers, strings, booleans, dates)
- Same scoping and filtering as JSON — no pagination (full result set)

### 3. Email Export (async job)

```rust
async fn email_report(
    State(app): State<AppContext>,
    CurrentActor(actor): CurrentActor,
    Query(request): Query<DatatableRequest>,
) -> impl IntoResponse {
    let result = InvoiceDatatable::queue_email(&app, Some(&actor), request, "alice@example.com").await?;
    Json(result)
}
```

Returns immediately:

```json
{ "datatable_id": "invoices", "recipient": "alice@example.com", "status": "queued" }
```

The XLSX is generated in a background job and delivered via `DatatableExportDelivery`. Register a custom delivery implementation:

```rust
struct EmailExportDelivery;

#[async_trait]
impl DatatableExportDelivery for EmailExportDelivery {
    async fn deliver(&self, export: GeneratedDatatableExport, recipient: &str) -> Result<()> {
        let email = EmailMessage::new(format!("{} Export", export.datatable_id))
            .to(EmailAddress::new(recipient))
            .attach(EmailAttachment::from_bytes(export.data, &export.filename));
        // send email...
        Ok(())
    }
}

// Register during bootstrap:
registrar.singleton(EmailExportDelivery as Arc<dyn DatatableExportDelivery>)?;
```

---

## Dynamic Datatable Resolution

Datatables are registered by ID and can be resolved dynamically:

```rust
// Generic handler that works for any registered datatable
async fn datatable_json(
    State(app): State<AppContext>,
    Path(datatable_id): Path<String>,
    CurrentActor(actor): CurrentActor,
    Query(request): Query<DatatableRequest>,
) -> impl IntoResponse {
    let registry = app.datatables()?;
    let datatable = registry.get(&datatable_id)
        .ok_or_else(|| Error::not_found(format!("datatable '{datatable_id}' not found")))?;

    Json(datatable.json(&app, Some(&actor), request).await?)
}
```

This lets you build a single `/api/datatables/:id` endpoint that serves all registered datatables.

---

## ProjectionDatatable (Grouped/Aggregate Reports)

For reports that need GROUP BY, aggregates, or don't map 1:1 to a model:

```rust
#[derive(Clone, Serialize, Projection)]
struct MonthlySalesReport {
    month: String,
    total_revenue: f64,
    order_count: i64,
}

struct MonthlySalesDatatable;

impl ProjectionDatatable for MonthlySalesDatatable {
    type Row = MonthlySalesReport;
    const ID: &'static str = "monthly-sales";

    fn query(ctx: &DatatableContext) -> ProjectionQuery<MonthlySalesReport> {
        ProjectionQuery::new()
            .group_by(MonthlySalesReport::MONTH)
    }

    fn columns() -> Vec<DatatableColumn<MonthlySalesReport>> {
        vec![
            DatatableColumn::field(MonthlySalesReport::MONTH).label("Month").sortable(),
            DatatableColumn::field(MonthlySalesReport::TOTAL_REVENUE).label("Revenue").sortable().exportable(),
            DatatableColumn::field(MonthlySalesReport::ORDER_COUNT).label("Orders").sortable().exportable(),
        ]
    }
}
```

---

## Complete Real-World Example

A full invoice datatable with scoping, filters, mappings, sorting, and all three output modes:

```rust
use forge::prelude::*;

// ── Model ──

#[derive(Model, Serialize)]
#[forge(table = "invoices")]
struct Invoice {
    id: ModelId<Self>,
    org_id: ModelId<Organization>,
    customer_name: String,
    amount: f64,
    currency: String,
    status: InvoiceStatus,
    due_date: Date,
    created_at: DateTime,
}

#[derive(Clone, Copy, AppEnum)]
enum InvoiceStatus {
    Draft,
    Sent,
    Paid,
    Overdue,
    Cancelled,
}

// ── Datatable ──

struct InvoiceDatatable;

impl ModelDatatable for InvoiceDatatable {
    type Model = Invoice;
    const ID: &'static str = "invoices";

    fn query(ctx: &DatatableContext) -> ModelQuery<Invoice> {
        let mut q = Invoice::model_query();
        // Scope to current user's organization
        if let Some(actor) = ctx.actor {
            if let Some(org) = actor.claims.get("org_id").and_then(|v| v.as_str()) {
                q = q.where_col(Invoice::ORG_ID, org);
            }
        }
        q
    }

    fn columns() -> Vec<DatatableColumn<Invoice>> {
        vec![
            DatatableColumn::field(Invoice::ID).label("Invoice").sortable().exportable(),
            DatatableColumn::field(Invoice::CUSTOMER_NAME).label("Customer").sortable().filterable().exportable(),
            DatatableColumn::field(Invoice::AMOUNT).label("Amount").sortable().filterable().exportable(),
            DatatableColumn::field(Invoice::CURRENCY).label("Currency").filterable(),
            DatatableColumn::field(Invoice::STATUS).label("Status").filterable().exportable(),
            DatatableColumn::field(Invoice::DUE_DATE).label("Due Date").sortable().filterable().exportable(),
            DatatableColumn::field(Invoice::CREATED_AT).label("Created").sortable().exportable(),
        ]
    }

    fn mappings() -> Vec<DatatableMapping<Invoice>> {
        vec![
            DatatableMapping::new("formatted_amount", |inv, _ctx| {
                DatatableValue::string(format!("{} {:.2}", inv.currency, inv.amount))
            }),
            DatatableMapping::new("is_overdue", |inv, _ctx| {
                let today = Date::parse(&DateTime::now().format()).unwrap_or_default();
                DatatableValue::bool(
                    matches!(inv.status, InvoiceStatus::Sent) && inv.due_date < today
                )
            }),
        ]
    }

    fn default_sort() -> Vec<DatatableSort<Invoice>> {
        vec![DatatableSort::desc(Invoice::CREATED_AT)]
    }

    async fn available_filters(_ctx: &DatatableContext) -> Result<Vec<DatatableFilterRow>> {
        Ok(vec![
            DatatableFilterRow::pair(
                DatatableFilterField::text("customer_name", "Customer")
                    .placeholder("Search customers..."),
                DatatableFilterField::select("status", "Status")
                    .options(vec![
                        DatatableFilterOption::new("draft", "Draft"),
                        DatatableFilterOption::new("sent", "Sent"),
                        DatatableFilterOption::new("paid", "Paid"),
                        DatatableFilterOption::new("overdue", "Overdue"),
                        DatatableFilterOption::new("cancelled", "Cancelled"),
                    ]),
            ),
            DatatableFilterRow::pair(
                DatatableFilterField::date("due_date", "Due Date"),
                DatatableFilterField::select("currency", "Currency")
                    .options(vec![
                        DatatableFilterOption::new("USD", "USD"),
                        DatatableFilterOption::new("EUR", "EUR"),
                        DatatableFilterOption::new("MYR", "MYR"),
                    ]),
            ),
        ])
    }
}

// ── Routes ──

fn routes(r: &mut HttpRegistrar) -> Result<()> {
    r.group("/invoices", |r| {
        r.route("", get(list_invoices));
        r.route("/download", get(download_invoices));
        r.route("/export", post(email_invoices));
        Ok(())
    })
}

async fn list_invoices(
    State(app): State<AppContext>,
    CurrentActor(actor): CurrentActor,
    Query(req): Query<DatatableRequest>,
) -> Result<impl IntoResponse> {
    Ok(Json(InvoiceDatatable::json(&app, Some(&actor), req).await?))
}

async fn download_invoices(
    State(app): State<AppContext>,
    CurrentActor(actor): CurrentActor,
    Query(req): Query<DatatableRequest>,
) -> Result<impl IntoResponse> {
    InvoiceDatatable::download(&app, Some(&actor), req).await
}

async fn email_invoices(
    State(app): State<AppContext>,
    CurrentActor(actor): CurrentActor,
    Query(req): Query<DatatableRequest>,
) -> Result<impl IntoResponse> {
    Ok(Json(InvoiceDatatable::queue_email(&app, Some(&actor), req, "finance@company.com").await?))
}
```

---

## Request Format

### Query Parameters (for GET requests)

```
GET /invoices?page=2&per_page=25&sort[0][field]=amount&sort[0][direction]=Desc&filters[0][field]=status&filters[0][op]=Eq&filters[0][value][Text]=paid
```

### JSON Body (for POST requests)

```json
{
  "page": 2,
  "per_page": 25,
  "sort": [{ "field": "amount", "direction": "Desc" }],
  "filters": [
    { "field": "status", "op": "Eq", "value": { "Text": "paid" } },
    { "field": "created_at", "op": "DateFrom", "value": { "Text": "2024-01-01" } }
  ],
  "search": "alice"
}
```

### DatatableRequest Helper Methods

Inside `filters()` or `query()`, use these to read request values:

```rust
ctx.request.text("customer_name")     // -> Option<&str>
ctx.request.bool("is_flagged")        // -> Option<bool>
ctx.request.date("due_date")          // -> Option<Date>
ctx.request.datetime("updated_at")    // -> Option<DateTime>
ctx.request.values("tags")            // -> Collection<String>
```
