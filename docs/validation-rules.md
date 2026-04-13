# Forge Validation Rules Reference

## Usage Pattern

```rust
use forge::prelude::*;

validator
    .field("email", &input.email)
    .bail()              // modifier: stop on first error
    .required()          // built-in: must not be empty
    .email()             // built-in: valid email format
    .unique("users", "email")  // built-in: DB check
    .apply()
    .await?;
```

All rules chain on `FieldValidator` and execute when `.apply()` is called.

---

## Modifiers

Modifiers change how the validation loop behaves. They are NOT rule steps — they're flags on the validator itself.

| Modifier | Effect |
|----------|--------|
| `.nullable()` | Skip all rules if the value is empty or whitespace |
| `.bail()` | Stop processing rules for this field after the first error |
| `.with_message(msg)` | Override the error message for the last added rule |

### nullable

Skip all validation when value is empty. Use for optional fields.

```rust
validator
    .field("nickname", &input.nickname)
    .nullable()
    .email()       // skipped if nickname is ""
    .min(3)        // skipped if nickname is ""
    .apply()
    .await?;
```

### bail

Stop on first error per field. Prevents error cascades.

```rust
validator
    .field("email", &input.email)
    .bail()
    .required()    // if this fails...
    .email()       // ...this is skipped
    .apply()
    .await?;
```

### with_message

Override the default error message for the preceding rule.

```rust
validator
    .field("age", &input.age)
    .required()
    .min_numeric(18.0)
    .with_message("You must be at least 18 years old")
    .apply()
    .await?;
```

---

## Built-in Rules

### Presence

| Rule | Code | Description |
|------|------|-------------|
| `.required()` | `required` | Value must not be empty or whitespace |

### String Rules

| Rule | Code | Description |
|------|------|-------------|
| `.min(n)` | `min` | String length must be at least `n` characters (Unicode chars, not bytes) |
| `.max(n)` | `max` | String length must be at most `n` characters (Unicode chars, not bytes) |
| `.alpha()` | `alpha` | Must contain only letters (a-z, A-Z, Unicode letters) |
| `.alpha_numeric()` | `alpha_numeric` | Must contain only letters and digits |
| `.digits()` | `digits` | Must contain only ASCII digits (0-9) |
| `.starts_with(prefix)` | `starts_with` | String must start with `prefix` |
| `.ends_with(suffix)` | `ends_with` | String must end with `suffix` |

### Numeric Rules

| Rule | Code | Description |
|------|------|-------------|
| `.numeric()` | `numeric` | Must be a numeric string (digits, optional `.`, `-`, `+`) |
| `.integer()` | `integer` | Must parse as a valid integer (`i64`) |
| `.min_numeric(n)` | `min_numeric` | Parsed number must be at least `n` |
| `.max_numeric(n)` | `max_numeric` | Parsed number must be at most `n` |
| `.between(min, max)` | `between` | Parsed number must be between `min` and `max` (inclusive) |

### Format Rules

| Rule | Code | Description |
|------|------|-------------|
| `.email()` | `email` | Must be a valid email address (RFC-compliant) |
| `.url()` | `url` | Must be a valid URL |
| `.uuid()` | `uuid` | Must be a valid UUID |
| `.regex(pattern)` | `regex` | Must match the given regex pattern |
| `.json()` | `json` | Must be valid JSON |
| `.timezone()` | `timezone` | Must be a valid timezone (UTC, IANA name, or offset like `+08:00`) |
| `.date()` | `date` | Must be a valid `YYYY-MM-DD` date |
| `.time()` | `time` | Must be a valid `HH:MM:SS` time |
| `.datetime()` | `datetime` | Must be a valid offset-aware datetime, or an offset-less datetime interpreted in the app timezone |
| `.local_datetime()` | `local_datetime` | Must be a valid timezone-less local datetime |

### IP Address Rules

| Rule | Code | Description |
|------|------|-------------|
| `.ip()` | `ip` | Must be a valid IP address (IPv4 or IPv6) |
| `.ipv4()` | `ipv4` | Must be a valid IPv4 address only |
| `.ipv6()` | `ipv6` | Must be a valid IPv6 address only |

### List Rules

| Rule | Code | Description |
|------|------|-------------|
| `.in_list([...])` | `in_list` | Value must be in the given list |
| `.not_in([...])` | `not_in` | Value must NOT be in the given list |

### Comparison Rules

| Rule | Code | Description |
|------|------|-------------|
| `.confirmed(field, value)` | `confirmed` | Value must match another field (e.g. password confirmation) |
| `.same(field, value)` | `same` | Value must match the given value |
| `.different(field, value)` | `different` | Value must differ from the given value |
| `.before(field, value)` | `before` | Value must be before the given temporal value |
| `.before_or_equal(field, value)` | `before_or_equal` | Value must be before or equal to the given temporal value |
| `.after(field, value)` | `after` | Value must be after the given temporal value |
| `.after_or_equal(field, value)` | `after_or_equal` | Value must be after or equal to the given temporal value |

Temporal comparison rules support `forge::DateTime`, `forge::LocalDateTime`, `forge::Date`, and `forge::Time` string formats. Offset-less `.datetime()` values are interpreted in the configured app timezone.

### Enum Rules

| Rule | Code | Description |
|------|------|-------------|
| `.app_enum::<E>()` | `app_enum` | Value must be a valid key in the given `ForgeAppEnum` type |

```rust
validator
    .field("status", &input.status)
    .required()
    .app_enum::<OrderStatus>()
    .apply()
    .await?;
```

### Database Rules (async)

These rules query the database. They require an active database connection via `AppContext`.

| Rule | Code | Description |
|------|------|-------------|
| `.unique(table, column)` | `unique` | Value must NOT exist in the given table/column |
| `.exists(table, column)` | `exists` | Value MUST exist in the given table/column |

```rust
validator
    .field("email", &input.email)
    .unique("users", "email")
    .apply()
    .await?;

validator
    .field("country_id", &input.country_id)
    .exists("countries", "id")
    .apply()
    .await?;
```

---

## Array / Collection Validation

Use `.each()` to validate each item in a collection. All field rules are available on the `EachValidator`.

```rust
validator
    .each("tags", &input.tags)
    .bail()
    .required()
    .min(2)
    .max(50)
    .apply()
    .await?;
```

Errors are reported with indexed field names (e.g. `tags.0`, `tags.1`).

---

## Validator Methods

The `Validator` struct provides these methods for controlling validation behavior:

| Method | Description |
|--------|-------------|
| `.locale(locale)` | Set locale for error message translation |
| `.set_locale(locale)` | Set locale (mutable version) |
| `.custom_message(field, code, message)` | Override error message for a specific field + rule |
| `.custom_attribute(field, name)` | Override display name for a field in error messages |
| `.add_error(field, code, params)` | Manually add a validation error |
| `.finish()` | Return `Ok(())` or `Err(ValidationErrors)` |

---

## Custom Rules

### Define

```rust
use async_trait::async_trait;
use forge::validation::{RuleContext, ValidationError, ValidationRule};

pub struct MobileRule;

#[async_trait]
impl ValidationRule for MobileRule {
    async fn validate(
        &self,
        context: &RuleContext,
        value: &str,
    ) -> std::result::Result<(), ValidationError> {
        // context.app() gives AppContext — access database, config, etc.
        if value.starts_with('+') && value[1..].chars().all(|c| c.is_ascii_digit()) {
            Ok(())
        } else {
            Err(ValidationError::new("mobile", "invalid mobile number"))
        }
    }
}
```

### Register

```rust
App::builder()
    .register_validation_rule("mobile", MobileRule)
    .run_http()?;
```

### Use

```rust
validator
    .field("phone", &input.phone)
    .required()
    .rule(ValidationRuleId::new("mobile"))
    .apply()
    .await?;
```

---

## File Upload Validation

Utility functions for validating uploaded files (not chained on `FieldValidator`):

| Function | Description |
|----------|-------------|
| `is_image(file)` | Check if file is an image (magic bytes) |
| `check_max_size(file, max_kb)` | Check file size <= max_kb KB |
| `get_image_dimensions(file)` | Get image (width, height) |
| `check_allowed_mimes(file, allowed)` | Check MIME type against allowed list |
| `check_allowed_extensions(file, allowed)` | Check extension against allowed list |

```rust
use forge::validation::file_rules;

if !file_rules::check_max_size(&file, 2048) {
    // file exceeds 2MB
}

if !file_rules::is_image(&file).await? {
    // not a valid image
}
```

---

## Request Validation (HTTP handlers)

### Define DTO with validator

```rust
#[derive(Deserialize)]
pub struct CreateUser {
    pub email: String,
    pub password: String,
    pub password_confirmation: String,
}

#[async_trait]
impl RequestValidator for CreateUser {
    async fn validate(&self, validator: &mut Validator) -> Result<()> {
        validator
            .field("email", &self.email)
            .bail()
            .required()
            .email()
            .unique("users", "email")
            .apply()
            .await?;

        validator
            .field("password", &self.password)
            .bail()
            .required()
            .min(8)
            .confirmed("password_confirmation", &self.password_confirmation)
            .apply()
            .await?;

        Ok(())
    }
}
```

### Use in route handler

```rust
async fn create_user(
    Validated(payload): Validated<CreateUser>,
) -> impl IntoResponse {
    // payload is validated — safe to use
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "email": payload.email })),
    )
}
```

### Error response format

```json
{
    "message": "Validation failed",
    "status": 422,
    "errors": [
        {
            "field": "email",
            "code": "required",
            "message": "email is required"
        },
        {
            "field": "password",
            "code": "min",
            "message": "password must be at least 8 characters"
        }
    ]
}
```

---

## Complete Rule Count

| Category | Count |
|----------|-------|
| Presence | 1 |
| String | 7 |
| Numeric | 5 |
| Format | 10 |
| IP | 3 |
| List | 2 |
| Comparison | 7 |
| Enum | 1 |
| Database (async) | 2 |
| Modifiers | 3 |
| Custom (user-defined) | unlimited |
| **Total built-in** | **38** |
