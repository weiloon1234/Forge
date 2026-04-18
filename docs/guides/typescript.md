# TypeScript Type Generation

Auto-generate TypeScript types from Rust structs and enums. Types are discovered at compile time — no manual registration.

---

## How It Works

Types that derive `ApiSchema`, `AppEnum`, or `forge::TS` are automatically registered for TypeScript export. Run `types:export` to generate `.ts` files.

```
Rust DTO (#[derive(ApiSchema)]) → auto-discovered → types:export → TypeScript file
```

---

## Quick Start

### 1. Derive on your types

Request/response DTOs already derive `ApiSchema` — they're auto-included:

```rust
#[derive(Debug, Deserialize, ts_rs::TS, forge::ApiSchema)]
#[ts(export)]
pub struct CreateOrderRequest {
    pub product_id: String,
    pub quantity: u32,
}
```

Enums that derive `AppEnum` — also auto-included:

```rust
#[derive(Clone, Debug, PartialEq, forge::AppEnum, ts_rs::TS)]
#[ts(export)]
pub enum OrderStatus {
    Pending,
    Confirmed,
    Shipped,
}
```

### 2. Run export

```bash
cargo run -- types:export
# or
make types
```

### 3. Use in frontend

```typescript
import type { CreateOrderRequest, OrderStatus } from "@shared/types/generated";
```

---

## Config

```toml
# config/typescript.toml
[typescript]
output_dir = "frontend/shared/types/generated"
```

Override via CLI flag:

```bash
cargo run -- types:export --output some/other/dir
```

Override via `.env`:

```
TYPESCRIPT__OUTPUT_DIR=frontend/shared/types/generated
```

---

## Derives

### `forge::ApiSchema` (request/response DTOs)

Auto-registers for TypeScript export. Must also derive `ts_rs::TS`:

```rust
#[derive(Debug, Deserialize, ts_rs::TS, forge::ApiSchema)]
#[ts(export)]
pub struct MyRequest { ... }
```

### `forge::AppEnum` (enums)

Auto-registers for TypeScript export. Must also derive `ts_rs::TS`:

```rust
#[derive(Clone, Debug, PartialEq, forge::AppEnum, ts_rs::TS)]
#[ts(export)]
pub enum MyEnum { ... }
```

### `forge::TS` (escape hatch)

For any type that isn't a DTO or AppEnum but needs TypeScript export:

```rust
#[derive(Serialize, ts_rs::TS, forge::TS)]
#[ts(export)]
pub struct SomeCustomType {
    pub name: String,
    pub value: f64,
}
```

---

## ts_rs Attributes

Control TypeScript output with `#[ts(...)]` attributes:

```rust
#[derive(Serialize, ts_rs::TS, forge::ApiSchema)]
#[ts(export)]
pub struct Example {
    pub name: String,

    #[ts(type = "number")]           // Override TS type
    pub count: u64,

    #[ts(optional)]                  // T | undefined
    pub nickname: Option<String>,

    #[ts(type = "Record<string, any>")]  // Complex type override
    pub metadata: serde_json::Value,
}
```

Common attributes:
- `#[ts(export)]` — mark for export (required)
- `#[ts(type = "...")]` — override generated TypeScript type
- `#[ts(optional)]` — make field optional (`T | undefined`)
- `#[ts(rename = "...")]` — rename in TypeScript output

---

## Framework Types

These types are auto-exported by the framework (no configuration needed):

| Type | Module | TypeScript |
|------|--------|------------|
| `CountryStatus` | `forge::countries` | `"enabled" \| "disabled"` |
| `TokenPair` | `forge::auth::token` | `{ access_token, refresh_token, ... }` |
| `RefreshTokenRequest` | `forge::auth::token` | `{ refresh_token }` |
| `TokenResponse` | `forge::auth::token` | `{ tokens: TokenPair }` |
| `MessageResponse` | `forge::http::response` | `{ message }` |
| `DatatableRequest` | `forge::datatable::request` | typed filters + sorts + pagination |
| `DatatableJsonResponse` | `forge::datatable::response` | typed columns + filters + applied filters + sorts |
| `JobHistoryStatus` | `forge::jobs` | `"succeeded" \| "retried" \| "dead_lettered"` |
| `SettingType` | `forge::settings` | `"text" \| "textarea" \| "number" \| ...` |

Datatable exports now keep JSON-facing numeric fields as `number` and include the supporting filter option imports needed by generated metadata files.

---

## Generated Output

```
frontend/shared/types/generated/
├── index.ts                    ← barrel (auto-generated)
├── CreateOrderRequest.ts       ← from project
├── OrderStatus.ts              ← from project
├── CountryStatus.ts            ← from framework
├── DatatableJsonResponse.ts    ← from framework
├── DatatableRequest.ts         ← from framework
├── MessageResponse.ts          ← from framework
├── RefreshTokenRequest.ts      ← from framework
├── TokenPair.ts                ← from framework
├── TokenResponse.ts            ← from framework
└── ...
```

The barrel `index.ts` re-exports all types:

```typescript
// Auto-generated barrel. Do not edit.
export type { CreateOrderRequest } from "./CreateOrderRequest";
export type { CountryStatus } from "./CountryStatus";
export type { DatatableJsonResponse } from "./DatatableJsonResponse";
export type { DatatableRequest } from "./DatatableRequest";
export type { MessageResponse } from "./MessageResponse";
export type { OrderStatus } from "./OrderStatus";
export type { RefreshTokenRequest } from "./RefreshTokenRequest";
export type { TokenPair } from "./TokenPair";
export type { TokenResponse } from "./TokenResponse";
```

---

## Integration with Makefile

```makefile
# Generate types (auto-discovered)
types:
    @PROCESS=cli cargo run -- types:export

# Dev: generates types before starting servers
dev: types
    ...

# Build: generates types before frontend build
build: types
    cd frontend/admin && npm run build
    cargo build --release
```

---

## Workflow

1. Add or modify a Rust struct/enum with `ApiSchema`, `AppEnum`, or `forge::TS`
2. Run `make types` (or `make dev` / `make build` which include it)
3. TypeScript types are generated — import and use in frontend

No registration files. No manual type lists. Derive → export → use.
