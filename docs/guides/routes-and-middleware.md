# Routes & Middleware Guide

HTTP routing, middleware stack, named URLs, API versioning, rate limiting, and more.

> For auth guards, permissions, and policies on routes, see [Auth Guide](auth.md).

---

## Quick Start

```rust
fn routes(r: &mut HttpRegistrar) -> Result<()> {
    r.route("/health", get(health));
    r.route("/posts", get(list_posts));
    r.route("/posts", post(create_post));
    Ok(())
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}
```

Register in bootstrap:

```rust
App::builder()
    .register_routes(routes)
    .run_http()?;
```

---

## Route Registration

### Basic Routes

```rust
fn routes(r: &mut HttpRegistrar) -> Result<()> {
    r.route("/posts", get(list_posts));
    r.route("/posts", post(create_post));
    r.route("/posts/:id", get(show_post));
    r.route("/posts/:id", put(update_post));
    r.route("/posts/:id", delete(delete_post));
    Ok(())
}
```

### Routes with Options

Attach guards, permissions, middleware, and rate limits to individual routes:

```rust
r.route_with_options("/posts", post(create_post),
    HttpRouteOptions::new()
        .guard(Guard::User)
        .permission(Permission::PostsWrite)
        .rate_limit(RateLimit::new(10).per_minute().by_actor()));
```

See [Auth Guide](auth.md) for setting up `Guard` and `Permission` enums.

### Route Groups

Prefix a set of routes without nesting into a separate router:

```rust
r.group("/admin", |r| {
    r.route("/dashboard", get(dashboard));     // /admin/dashboard
    r.route("/users", get(admin_users));        // /admin/users
    r.route("/users/:id", get(admin_user));     // /admin/users/:id
    Ok(())
})?;
```

Groups can nest:

```rust
r.group("/api", |r| {
    r.group("/v1", |r| {
        r.route("/posts", get(v1_posts));       // /api/v1/posts
        Ok(())
    })?;
    Ok(())
})?;
```

### API Versioning

Shorthand for `/api/v{N}` groups:

```rust
r.api_version(1, |r| {
    r.route("/users", get(list_users_v1));     // /api/v1/users
    r.route("/posts", get(list_posts_v1));     // /api/v1/posts
    Ok(())
})?;

r.api_version(2, |r| {
    r.route("/users", get(list_users_v2));     // /api/v2/users
    Ok(())
})?;
```

### Nest & Merge

For integrating external Axum routers:

```rust
// Nest under a prefix
let admin_router = Router::new().route("/stats", get(stats));
r.nest("/admin", admin_router);  // /admin/stats

// Merge at the same level (no prefix)
let health_router = Router::new().route("/healthz", get(healthz));
r.merge(health_router);  // /healthz
```

---

## Named Routes & URL Generation

### Registering Named Routes

```rust
r.route_named("posts.list", "/posts", get(list_posts));
r.route_named("posts.show", "/posts/:id", get(show_post));
r.route_named("password.reset", "/reset/:token", get(reset_form));

// Named + options
r.route_named_with_options("posts.create", "/posts", post(create_post),
    HttpRouteOptions::new().guard(Guard::User));
```

### Generating URLs

In a handler:

```rust
async fn some_handler(State(app): State<AppContext>) -> Result<impl IntoResponse> {
    let url = app.route_url("posts.show", &[("id", "42")])?;
    // → "/posts/42"

    Ok(Json(json!({ "url": url })))
}
```

### Signed URLs

Generate tamper-proof URLs with expiry (for password resets, email verification, etc.):

```rust
let url = app.signed_route_url(
    "password.reset",
    &[("token", &reset_token)],
    DateTime::now().add_days(1),  // expires in 24 hours
)?;
// → "/reset/abc123?expires=1704067200&signature=hmac_sha256..."
```

Verify in a handler:

```rust
async fn reset_form(State(app): State<AppContext>, request: Request) -> Result<impl IntoResponse> {
    app.verify_signed_url(&request.uri().to_string())?;
    // Returns Error if expired or tampered
    Ok(Html("reset form"))
}
```

---

## Middleware Stack

### Global Middleware

Register middleware that runs on every request:

```rust
App::builder()
    .register_middleware(MiddlewareConfig::from(Compression))
    .register_middleware(MiddlewareConfig::from(
        SecurityHeaders::new()
            .content_security_policy("default-src 'self'")
    ))
    .register_middleware(MiddlewareConfig::from(
        Cors::new()
            .allow_origins(["https://app.example.com"])
            .allow_credentials()
    ))
    .run_http()?;
```

### Execution Order

Middleware runs in **priority order** — lower numbers run first (outermost layer):

```
Request
  │
  ├─ 0  TrustedProxy         ← extract real IP from proxy headers
  ├─ 1  MaintenanceMode      ← return 503 if in maintenance
  ├─ 10 Cors                 ← handle CORS preflight
  ├─ 20 SecurityHeaders      ← add security headers
  ├─ 25 Csrf                 ← validate CSRF tokens
  ├─ 30 RateLimit            ← check rate limits
  ├─ 40 MaxBodySize          ← enforce body size
  ├─ 50 RequestTimeout       ← enforce timeout
  ├─ 55 ETag                 ← conditional response (304)
  ├─ 60 Compression          ← compress response
  │
  ├─ [per-route middleware]   ← from HttpRouteOptions
  ├─ [auth middleware]        ← if route requires guard
  │
  └─ Handler
```

You don't need to worry about order — the framework sorts by priority automatically.

---

## Middleware Reference

### Compression

Gzip + Brotli based on `Accept-Encoding`:

```rust
MiddlewareConfig::from(Compression)
```

### CORS

```rust
MiddlewareConfig::from(
    Cors::new()
        .allow_origins(["https://app.example.com", "https://admin.example.com"])
        .allow_any_method()
        .allow_any_header()
        .allow_credentials()
        .max_age(3600)
)
```

For development:

```rust
MiddlewareConfig::from(Cors::new().allow_any_origin())
```

### Security Headers

Adds HSTS, CSP, X-Frame-Options, X-Content-Type-Options, Referrer-Policy:

```rust
MiddlewareConfig::from(
    SecurityHeaders::new()
        .content_security_policy("default-src 'self'; script-src 'self' 'unsafe-inline'")
        .frame_options("SAMEORIGIN")
)
```

Defaults (applied without any builder calls):

| Header | Default Value |
|--------|--------------|
| `Strict-Transport-Security` | `max-age=31536000; includeSubDomains` |
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `DENY` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |
| `X-XSS-Protection` | `0` |

### CSRF

Double-submit cookie pattern for state-changing requests:

```rust
MiddlewareConfig::from(
    Csrf::new()
        .exclude("/api")       // skip CSRF for API routes (use token auth instead)
)
```

**How it works:**

- GET/HEAD/OPTIONS → generates CSRF token, sets cookie (readable by JS)
- POST/PUT/PATCH/DELETE → validates `X-CSRF-Token` header matches cookie
- Returns 403 if mismatch

**Frontend integration:**

```javascript
const token = document.cookie.split('; ')
    .find(row => row.startsWith('csrf_token='))?.split('=')[1];

fetch('/form', {
    method: 'POST',
    headers: { 'X-CSRF-Token': token },
    body: formData,
});
```

**Extract token in handler** (e.g., to embed in HTML form):

```rust
async fn form(CsrfToken(token): CsrfToken) -> impl IntoResponse {
    Html(format!(r#"<input type="hidden" name="_token" value="{token}">"#))
}
```

### Rate Limiting

```rust
// Global: 1000 requests per hour per IP
MiddlewareConfig::from(RateLimit::new(1000).per_hour())

// Per-route: 10 per minute per authenticated user
HttpRouteOptions::new()
    .rate_limit(RateLimit::new(10).per_minute().by_actor())
```

**Strategies:**

| Method | Key | Auth required? | Use case |
|--------|-----|---------------|----------|
| (default) | Client IP | No | Global rate limit |
| `.by_actor()` | Actor ID | Yes | Per-user limits |
| `.by_actor_or_ip()` | Actor ID or IP | No | Actor if authenticated, IP as fallback |

**Response headers on rate-limited requests:**

```
X-RateLimit-Limit: 1000
X-RateLimit-Remaining: 42
X-RateLimit-Reset: 1704067200
```

On limit exceeded: **429 Too Many Requests** with `Retry-After` header.

### Max Body Size

```rust
MiddlewareConfig::from(MaxBodySize::mb(10))    // 10 MB limit
MiddlewareConfig::from(MaxBodySize::kb(512))   // 512 KB limit
MiddlewareConfig::from(MaxBodySize::bytes(1024)) // 1024 bytes
```

Returns **413 Payload Too Large** if exceeded.

### Request Timeout

```rust
MiddlewareConfig::from(RequestTimeout::secs(30))
MiddlewareConfig::from(RequestTimeout::mins(5))
```

Returns **408 Request Timeout** if exceeded.

### ETag

Automatic conditional responses — returns 304 Not Modified when content hasn't changed:

```rust
MiddlewareConfig::from(ETag::new())
```

Computes SHA-256 of response body. If client sends `If-None-Match` header matching the ETag, returns 304 with no body. Skips responses larger than 10 MB.

### Trusted Proxy

Extract real client IP from proxy headers:

```rust
MiddlewareConfig::from(TrustedProxy::cloudflare())
```

Resolution order: `CF-Connecting-IP` → `X-Real-IP` → `X-Forwarded-For` (first entry).

The resolved IP is available via the `RealIp` extractor:

```rust
async fn handler(RealIp(ip): RealIp) -> impl IntoResponse {
    Json(json!({ "your_ip": ip.to_string() }))
}
```

### Maintenance Mode

Returns 503 for all requests (bypassed with secret):

```rust
MiddlewareConfig::from(
    MaintenanceMode::new()
        .bypass_secret("my-secret")
)
```

**CLI commands:**

```bash
cargo run -- down --secret=my-secret    # enter maintenance mode
cargo run -- up                          # exit maintenance mode
```

**Bypass via header:**

```bash
curl -H "X-Maintenance-Bypass: my-secret" https://app.example.com
```

---

## Middleware Groups

Define a named bundle of middleware once, apply to multiple routes by name:

### Define

```rust
App::builder()
    .middleware_group("api", vec![
        MiddlewareConfig::from(RateLimit::new(1000).per_hour()),
        MiddlewareConfig::from(Compression),
    ])
    .middleware_group("web", vec![
        MiddlewareConfig::from(Csrf::new()),
        MiddlewareConfig::from(SecurityHeaders::new()),
        MiddlewareConfig::from(Compression),
    ])
```

### Apply

```rust
fn routes(r: &mut HttpRegistrar) -> Result<()> {
    // API routes get "api" group middleware
    r.api_version(1, |r| {
        r.route_with_options("/users", get(list_users),
            HttpRouteOptions::new()
                .guard(Guard::User)
                .middleware_group("api"));
        Ok(())
    })?;

    // Web routes get "web" group middleware
    r.group("/dashboard", |r| {
        r.route_with_options("/", get(dashboard),
            HttpRouteOptions::new()
                .guard(Guard::Admin)
                .middleware_group("web"));
        Ok(())
    })?;

    Ok(())
}
```

Group middleware is prepended before any per-route middleware. You can combine a group with additional per-route middleware:

```rust
HttpRouteOptions::new()
    .middleware_group("api")
    .middleware(MiddlewareConfig::from(MaxBodySize::mb(50)))  // on top of group
```

---

## Per-Route Middleware

Apply middleware to a single route via `HttpRouteOptions`:

```rust
r.route_with_options("/upload", post(upload_file),
    HttpRouteOptions::new()
        .guard(Guard::User)
        .middleware(MiddlewareConfig::from(MaxBodySize::mb(100)))
        .middleware(MiddlewareConfig::from(RequestTimeout::mins(5))));
```

Per-route middleware runs **after** global middleware and **before** the auth check.

---

## SPA Serving

Serve a frontend SPA (React, Vue, etc.) with client-side routing fallback:

```rust
App::builder()
    .serve_spa("frontend/dist")
    .register_routes(api_routes)
    .run_http()?;
```

All requests not matched by API routes fall back to `frontend/dist/index.html`. Static assets (JS, CSS, images) are served directly from the directory.

---

## Route Listing

See all named routes:

```bash
cargo run -- routes:list
```

```
NAME                           PATH
posts.list                     /api/v1/posts
posts.show                     /api/v1/posts/:id
posts.create                   /api/v1/posts
password.reset                 /reset/:token
admin.dashboard                /admin/dashboard
```

---

## API Resources

Transform models into consistent JSON response shapes:

```rust
struct UserResource;

impl ApiResource<User> for UserResource {
    fn transform(user: &User) -> Value {
        json!({
            "id": user.id,
            "email": user.email,
            "name": user.name,
            "joined": user.created_at.format(),
        })
    }
}
```

Use in handlers:

```rust
async fn list_users(State(app): State<AppContext>) -> impl IntoResponse {
    let db = app.database()?;
    let users = User::model_query().all(&*db).await?;
    Json(UserResource::collection(&users))
}

async fn show_user(State(app): State<AppContext>, Path(id): Path<String>) -> impl IntoResponse {
    let db = app.database()?;
    let user = User::model_query()
        .where_col(User::ID, &id)
        .first(&*db).await?
        .ok_or_else(|| Error::not_found("user not found"))?;
    Json(UserResource::make(&user))
}

// Paginated response with meta + links
async fn paginated_users(State(app): State<AppContext>, Query(page): Query<Pagination>) -> impl IntoResponse {
    let db = app.database()?;
    let paginated = User::model_query()
        .paginate(page, &*db).await?;
    Json(UserResource::paginated(&paginated, "/api/v1/users"))
}
```

---

## Complete Example

```rust
use forge::prelude::*;

fn main() -> Result<()> {
    App::builder()
        .load_env()
        .load_config_dir("config")
        .register_provider(AppServiceProvider)

        // Global middleware
        .register_middleware(MiddlewareConfig::from(TrustedProxy::cloudflare()))
        .register_middleware(MiddlewareConfig::from(Compression))
        .register_middleware(MiddlewareConfig::from(
            SecurityHeaders::new()
                .content_security_policy("default-src 'self'")
        ))

        // Middleware groups
        .middleware_group("api", vec![
            MiddlewareConfig::from(RateLimit::new(1000).per_hour()),
        ])
        .middleware_group("web", vec![
            MiddlewareConfig::from(Csrf::new().exclude("/api")),
        ])

        // SPA frontend
        .serve_spa("frontend/dist")

        // Routes
        .register_routes(routes)
        .run_http()
}

fn routes(r: &mut HttpRegistrar) -> Result<()> {
    // Public
    r.route("/health", get(|| async { Json(json!({ "ok": true })) }));

    // API v1
    r.api_version(1, |r| {
        r.route_named("posts.list", "/posts", get(list_posts));

        r.route_named_with_options("posts.create", "/posts", post(create_post),
            HttpRouteOptions::new()
                .guard(Guard::User)
                .permission(Permission::PostsWrite)
                .middleware_group("api")
                .rate_limit(RateLimit::new(30).per_minute().by_actor()));

        r.route_named_with_options("posts.show", "/posts/:id", get(show_post),
            HttpRouteOptions::new()
                .guard(Guard::User)
                .middleware_group("api"));

        Ok(())
    })?;

    // Admin dashboard
    r.group("/admin", |r| {
        r.route_with_options("/", get(dashboard),
            HttpRouteOptions::new()
                .guard(Guard::Admin)
                .permission(Permission::AdminAccess)
                .middleware_group("web"));
        Ok(())
    })?;

    Ok(())
}
```
