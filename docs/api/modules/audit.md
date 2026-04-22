# audit

[Back to index](../index.md)

## forge::audit

```rust
struct AuditLog
  const ID: Column<Self, ModelId<AuditLog>>
  const EVENT_TYPE: Column<Self, String>
  const SUBJECT_MODEL: Column<Self, String>
  const SUBJECT_TABLE: Column<Self, String>
  const SUBJECT_ID: Column<Self, String>
  const ACTOR_GUARD: Column<Self, Option<String>>
  const ACTOR_ID: Column<Self, Option<String>>
  const REQUEST_ID: Column<Self, Option<String>>
  const IP: Column<Self, Option<String>>
  const USER_AGENT: Column<Self, Option<String>>
  const BEFORE_DATA: Column<Self, Option<Value>>
  const AFTER_DATA: Column<Self, Option<Value>>
  const CHANGES: Column<Self, Option<Value>>
  const CREATED_AT: Column<Self, DateTime>
  fn query() -> ModelQuery<Self>
  fn create() -> CreateModel<Self>
  fn create_many() -> CreateManyModel<Self>
  fn update() -> UpdateModel<Self>
  fn delete() -> DeleteModel<Self>
  fn force_delete() -> DeleteModel<Self>
  fn restore() -> RestoreModel<Self>
```

