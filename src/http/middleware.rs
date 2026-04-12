use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use crate::foundation::AppContext;
use crate::logging::RuntimeBackendKind;
use crate::support::runtime::RuntimeBackend;

// ---------------------------------------------------------------------------
// RealIp extension
// ---------------------------------------------------------------------------

/// Extension stored by `TrustedProxy` middleware carrying the resolved client IP.
#[derive(Clone, Debug)]
pub struct RealIp(pub IpAddr);

// ---------------------------------------------------------------------------
// MiddlewareConfig — enum of all middleware types
// ---------------------------------------------------------------------------

/// Enumerates all Forge middleware types with their configuration.
///
/// Each variant knows its priority for ordering and can be applied to a router.
/// Consumers never construct this directly — they use the individual builder
/// types (`Cors`, `SecurityHeaders`, etc.) which convert into `MiddlewareConfig`.
#[derive(Clone, Debug)]
pub enum MiddlewareConfig {
    TrustedProxy(TrustedProxy),
    MaintenanceMode(MaintenanceMode),
    Cors(Cors),
    SecurityHeaders(SecurityHeaders),
    Csrf(Csrf),
    RateLimit(RateLimit),
    MaxBodySize(MaxBodySize),
    RequestTimeout(RequestTimeout),
    ETag(ETag),
    Compression(Compression),
}

impl MiddlewareConfig {
    /// Priority for ordering: lower values are applied first (outermost layer).
    pub(crate) fn priority(&self) -> u8 {
        match self {
            Self::TrustedProxy(_) => 0,
            Self::MaintenanceMode(_) => 1,
            Self::Cors(_) => 10,
            Self::SecurityHeaders(_) => 20,
            Self::Csrf(_) => 25,
            Self::RateLimit(_) => 30,
            Self::MaxBodySize(_) => 40,
            Self::RequestTimeout(_) => 50,
            Self::ETag(_) => 55,
            Self::Compression(_) => 60,
        }
    }

    /// Apply this middleware to the given router.
    pub(crate) fn apply(
        self,
        router: axum::Router<AppContext>,
        app: &AppContext,
    ) -> axum::Router<AppContext> {
        match self {
            Self::TrustedProxy(config) => config.apply(router),
            Self::MaintenanceMode(config) => config.apply(router, app),
            Self::Cors(config) => config.apply(router),
            Self::SecurityHeaders(config) => config.apply(router),
            Self::Csrf(config) => config.apply(router),
            Self::RateLimit(config) => config.apply(router, app),
            Self::MaxBodySize(config) => config.apply(router),
            Self::RequestTimeout(config) => config.apply(router),
            Self::ETag(config) => config.apply(router),
            Self::Compression(config) => config.apply(router),
        }
    }
}

// ---------------------------------------------------------------------------
// apply_ordered_middlewares
// ---------------------------------------------------------------------------

/// Sort middleware configs by priority (ascending) and apply them to the router.
///
/// Lower priority values wrap the router first, so they become the outermost
/// layers and run first on incoming requests.
pub(crate) fn apply_ordered_middlewares(
    mut router: axum::Router<AppContext>,
    mut middlewares: Vec<MiddlewareConfig>,
    app: &AppContext,
) -> axum::Router<AppContext> {
    middlewares.sort_by_key(|m| m.priority());
    for mw in middlewares {
        router = mw.apply(router, app);
    }
    router
}

// ---------------------------------------------------------------------------
// Cors
// ---------------------------------------------------------------------------

/// CORS middleware configuration.
///
/// Wraps `tower_http::cors::CorsLayer` with a builder API.
///
/// ```
/// use forge::http::middleware::Cors;
///
/// let cors = Cors::new()
///     .allow_any_origin()
///     .allow_any_method()
///     .allow_headers([axum::http::header::CONTENT_TYPE]);
/// ```
#[derive(Clone, Debug)]
pub struct Cors {
    origins: CorsOrigins,
    methods: CorsMethods,
    headers: CorsHeaders,
    credentials: bool,
    max_age: Option<Duration>,
}

#[derive(Clone, Debug)]
enum CorsOrigins {
    None,
    Any,
    List(Vec<String>),
}

#[derive(Clone, Debug)]
enum CorsMethods {
    None,
    Any,
    List(Vec<Method>),
}

#[derive(Clone, Debug)]
enum CorsHeaders {
    None,
    Any,
    List(Vec<HeaderName>),
}

impl Cors {
    /// Create a new CORS configuration with no origins, methods, or headers allowed.
    pub fn new() -> Self {
        Self {
            origins: CorsOrigins::None,
            methods: CorsMethods::None,
            headers: CorsHeaders::None,
            credentials: false,
            max_age: None,
        }
    }

    /// Allow a single origin.
    pub fn allow_origin(mut self, origin: &str) -> Self {
        self.origins = CorsOrigins::List(vec![origin.to_string()]);
        self
    }

    /// Allow multiple origins.
    pub fn allow_origins<I, O>(mut self, origins: I) -> Self
    where
        I: IntoIterator<Item = O>,
        O: AsRef<str>,
    {
        self.origins = CorsOrigins::List(
            origins
                .into_iter()
                .map(|o| o.as_ref().to_string())
                .collect(),
        );
        self
    }

    /// Allow any origin.
    pub fn allow_any_origin(mut self) -> Self {
        self.origins = CorsOrigins::Any;
        self
    }

    /// Allow a single HTTP method.
    pub fn allow_method(mut self, method: Method) -> Self {
        self.methods = CorsMethods::List(vec![method]);
        self
    }

    /// Allow multiple HTTP methods.
    pub fn allow_methods<I>(mut self, methods: I) -> Self
    where
        I: IntoIterator<Item = Method>,
    {
        self.methods = CorsMethods::List(methods.into_iter().collect());
        self
    }

    /// Allow any HTTP method.
    pub fn allow_any_method(mut self) -> Self {
        self.methods = CorsMethods::Any;
        self
    }

    /// Allow a single request header.
    pub fn allow_header(mut self, hdr: HeaderName) -> Self {
        self.headers = CorsHeaders::List(vec![hdr]);
        self
    }

    /// Allow multiple request headers.
    pub fn allow_headers<I>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = HeaderName>,
    {
        self.headers = CorsHeaders::List(headers.into_iter().collect());
        self
    }

    /// Allow any request header.
    pub fn allow_any_header(mut self) -> Self {
        self.headers = CorsHeaders::Any;
        self
    }

    /// Include `Access-Control-Allow-Credentials: true`.
    pub fn allow_credentials(mut self) -> Self {
        self.credentials = true;
        self
    }

    /// Set `Access-Control-Max-Age` in seconds.
    pub fn max_age(mut self, seconds: u64) -> Self {
        self.max_age = Some(Duration::from_secs(seconds));
        self
    }

    /// Convert into a `MiddlewareConfig`.
    pub fn build(self) -> MiddlewareConfig {
        MiddlewareConfig::Cors(self)
    }

    fn apply(self, router: axum::Router<AppContext>) -> axum::Router<AppContext> {
        let mut layer = CorsLayer::new();

        layer = match self.origins {
            CorsOrigins::None => layer,
            CorsOrigins::Any => layer.allow_origin(tower_http::cors::Any),
            CorsOrigins::List(ref origins) if origins.len() == 1 => {
                if let Ok(value) = HeaderValue::from_str(&origins[0]) {
                    layer.allow_origin(value)
                } else {
                    layer
                }
            }
            CorsOrigins::List(ref origins) => {
                let values: Vec<HeaderValue> = origins
                    .iter()
                    .filter_map(|o| HeaderValue::from_str(o).ok())
                    .collect();
                layer.allow_origin(values)
            }
        };

        layer = match self.methods {
            CorsMethods::None => layer,
            CorsMethods::Any => layer.allow_methods(tower_http::cors::Any),
            CorsMethods::List(methods) => layer.allow_methods(methods),
        };

        layer = match self.headers {
            CorsHeaders::None => layer,
            CorsHeaders::Any => layer.allow_headers(tower_http::cors::Any),
            CorsHeaders::List(headers) => layer.allow_headers(headers),
        };

        if self.credentials {
            layer = layer.allow_credentials(true);
        }

        if let Some(duration) = self.max_age {
            layer = layer.max_age(duration);
        }

        router.layer(layer)
    }
}

impl Default for Cors {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SecurityHeaders
// ---------------------------------------------------------------------------

const HSTS_HEADER: HeaderName = header::STRICT_TRANSPORT_SECURITY;
const X_CONTENT_TYPE_OPTIONS: HeaderName = header::X_CONTENT_TYPE_OPTIONS;
const X_FRAME_OPTIONS: HeaderName = header::X_FRAME_OPTIONS;
const REFERRER_POLICY: HeaderName = header::REFERRER_POLICY;
const X_XSS_PROTECTION: HeaderName = HeaderName::from_static("x-xss-protection");

/// Security headers middleware.
///
/// Adds security-related headers to every response. All defaults are applied
/// on construction and can be customised via builder methods.
///
/// Default headers:
/// - `X-Content-Type-Options: nosniff`
/// - `X-Frame-Options: DENY`
/// - `Strict-Transport-Security: max-age=31536000; includeSubDomains`
/// - `Referrer-Policy: strict-origin-when-cross-origin`
/// - `X-XSS-Protection: 0`
#[derive(Clone, Debug)]
pub struct SecurityHeaders {
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl SecurityHeaders {
    /// Create with all default security headers.
    pub fn new() -> Self {
        Self {
            headers: vec![
                (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
                (X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
                (
                    HSTS_HEADER,
                    HeaderValue::from_static("max-age=31536000; includeSubDomains"),
                ),
                (
                    REFERRER_POLICY,
                    HeaderValue::from_static("strict-origin-when-cross-origin"),
                ),
                (X_XSS_PROTECTION, HeaderValue::from_static("0")),
            ],
        }
    }

    /// Disable the `Strict-Transport-Security` header.
    pub fn disable_hsts(mut self) -> Self {
        self.headers.retain(|(name, _)| *name != HSTS_HEADER);
        self
    }

    /// Set the `X-Frame-Options` value.
    pub fn frame_options(mut self, value: &str) -> Self {
        if let Ok(hv) = HeaderValue::from_str(value) {
            if let Some(entry) = self.headers.iter_mut().find(|(n, _)| *n == X_FRAME_OPTIONS) {
                entry.1 = hv;
            }
        }
        self
    }

    /// Add a `Content-Security-Policy` header. Invalid values are silently skipped.
    pub fn content_security_policy(mut self, policy: &str) -> Self {
        if let Ok(hv) = HeaderValue::from_str(policy) {
            self = self.header(header::CONTENT_SECURITY_POLICY, hv);
        }
        self
    }

    /// Set the `Referrer-Policy` value.
    pub fn referrer_policy(mut self, policy: &str) -> Self {
        if let Ok(hv) = HeaderValue::from_str(policy) {
            if let Some(entry) = self.headers.iter_mut().find(|(n, _)| *n == REFERRER_POLICY) {
                entry.1 = hv;
            }
        }
        self
    }

    /// Add a custom header to every response.
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.push((name, value));
        self
    }

    /// Convert into a `MiddlewareConfig`.
    pub fn build(self) -> MiddlewareConfig {
        MiddlewareConfig::SecurityHeaders(self)
    }

    fn apply(self, router: axum::Router<AppContext>) -> axum::Router<AppContext> {
        let headers = self.headers;
        router.layer(middleware::from_fn(move |request: Request, next: Next| {
            let headers = headers.clone();
            async move { security_headers_fn(request, next, &headers).await }
        }))
    }
}

async fn security_headers_fn(
    request: Request,
    next: Next,
    headers: &[(HeaderName, HeaderValue)],
) -> Response {
    let mut response = next.run(request).await;
    let response_headers = response.headers_mut();
    for (name, value) in headers {
        response_headers.insert(name.clone(), value.clone());
    }
    response
}

impl Default for SecurityHeaders {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Csrf
// ---------------------------------------------------------------------------

/// CSRF protection middleware using the double-submit cookie pattern.
///
/// Generates a token stored in a cookie and validates that state-changing
/// requests (POST/PUT/PATCH/DELETE) include the matching token in a header
/// or form field. GET/HEAD/OPTIONS requests are exempt.
#[derive(Clone, Debug)]
pub struct Csrf {
    cookie_name: String,
    header_name: HeaderName,
    secure: bool,
    exclude: Vec<String>,
}

impl Csrf {
    pub fn new() -> Self {
        Self {
            cookie_name: "forge_csrf".to_string(),
            header_name: HeaderName::from_static("x-csrf-token"),
            secure: true,
            exclude: Vec::new(),
        }
    }

    pub fn cookie_name(mut self, name: &str) -> Self {
        self.cookie_name = name.to_string();
        self
    }

    pub fn header_name(mut self, name: HeaderName) -> Self {
        self.header_name = name;
        self
    }

    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    /// Add a path prefix to exclude from CSRF validation (e.g., "/api").
    pub fn exclude(mut self, path: &str) -> Self {
        self.exclude.push(path.to_string());
        self
    }

    pub fn build(self) -> MiddlewareConfig {
        MiddlewareConfig::Csrf(self)
    }

    fn apply(self, router: axum::Router<AppContext>) -> axum::Router<AppContext> {
        let state = CsrfState {
            cookie_name: self.cookie_name,
            header_name: self.header_name,
            secure: self.secure,
            exclude: self.exclude,
        };
        router.layer(middleware::from_fn_with_state(state, csrf_middleware))
    }
}

impl Default for Csrf {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
struct CsrfState {
    cookie_name: String,
    header_name: HeaderName,
    secure: bool,
    exclude: Vec<String>,
}

/// Extension inserted by the CSRF middleware containing the current token.
#[derive(Clone, Debug)]
pub struct CsrfToken(String);

impl CsrfToken {
    /// The CSRF token value (for rendering in forms or meta tags).
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl<S> axum::extract::FromRequestParts<S> for CsrfToken
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<CsrfToken>()
            .cloned()
            .ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({
                        "message": "CSRF middleware not active on this route",
                        "status": 500
                    })),
                )
                    .into_response()
            })
    }
}

async fn csrf_middleware(
    State(state): State<CsrfState>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // Check if path is excluded
    if state.exclude.iter().any(|prefix| path.starts_with(prefix)) {
        return next.run(request).await;
    }

    let method = request.method().clone();
    let is_safe = matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);

    // Extract existing token from cookie
    let existing_token = extract_cookie_value(request.headers(), &state.cookie_name);

    if is_safe {
        // Safe methods: ensure token cookie exists, set extension
        let token = match existing_token {
            Some(ref token) => token.clone(),
            None => {
                // Generate new token
                match crate::support::Token::base64(32) {
                    Ok(t) => t,
                    Err(_) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            axum::Json(serde_json::json!({
                                "message": "Failed to generate CSRF token",
                                "status": 500
                            })),
                        )
                            .into_response();
                    }
                }
            }
        };

        request.extensions_mut().insert(CsrfToken(token.clone()));
        let mut response = next.run(request).await;

        // Set cookie if it wasn't present
        if existing_token.is_none() {
            let cookie = build_csrf_cookie(&state.cookie_name, &token, state.secure);
            if let Ok(hv) = cookie.parse::<HeaderValue>() {
                response.headers_mut().append(header::SET_COOKIE, hv);
            }
        }

        response
    } else {
        // State-changing methods: validate token
        let Some(cookie_token) = existing_token else {
            return csrf_forbidden("CSRF token cookie missing");
        };

        // Check header first, then query for the form field value won't work easily
        // without consuming the body. For API-first framework, header is primary.
        let request_token = request
            .headers()
            .get(&state.header_name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let Some(request_token) = request_token else {
            return csrf_forbidden("CSRF token missing from request header");
        };

        if !crate::support::hmac::constant_time_eq(cookie_token.as_bytes(), request_token.as_bytes()) {
            return csrf_forbidden("CSRF token mismatch");
        }

        request.extensions_mut().insert(CsrfToken(cookie_token));
        next.run(request).await
    }
}

/// Build a CSRF cookie string. Intentionally NOT HttpOnly — the frontend JS must
/// read this cookie to include the token in the X-CSRF-TOKEN request header
/// (double-submit cookie pattern).
fn build_csrf_cookie(name: &str, value: &str, secure: bool) -> String {
    let mut cookie = format!("{name}={value}; Path=/; SameSite=Lax");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn csrf_forbidden(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({
            "message": message,
            "status": 403
        })),
    )
        .into_response()
}

// extract_cookie_value is in crate::http::cookie (shared with session auth)
use super::cookie::extract_cookie_value;

// ---------------------------------------------------------------------------
// RateLimit
// ---------------------------------------------------------------------------

/// The time window for rate limiting.
#[derive(Clone, Copy, Debug)]
pub enum RateLimitWindow {
    Second,
    Minute,
    Hour,
}

impl RateLimitWindow {
    fn duration_secs(&self) -> u64 {
        match self {
            Self::Second => 1,
            Self::Minute => 60,
            Self::Hour => 3600,
        }
    }
}

/// Determines how rate-limit keys are derived.
#[derive(Clone, Copy, Debug, Default)]
pub enum RateLimitBy {
    /// Key by client IP address (default).
    #[default]
    Ip,
    /// Key by authenticated actor ID (requires auth middleware).
    Actor,
    /// Key by actor ID when authenticated, falling back to IP.
    ActorOrIp,
}

/// Rate-limit store backend.
#[derive(Clone)]
pub(crate) enum RateLimitStore {
    /// In-memory fixed-window counter. Used when Redis is not configured.
    Memory(Arc<Mutex<HashMap<String, (u32, u64)>>>),
    /// Redis-backed counter via `INCR` + `EXPIRE`. Used automatically when
    /// the runtime backend is Redis.
    Redis(RuntimeBackend),
}

/// Fixed-window rate limiter with Redis-backed storage.
///
/// Uses Redis automatically when configured, falls back to in-memory storage
/// for development and testing.
///
/// ```
/// use forge::http::middleware::RateLimit;
///
/// let limiter = RateLimit::new(100)
///     .per_minute()
///     .key_prefix("my_api:");
/// ```
#[derive(Clone, Debug)]
pub struct RateLimit {
    max: u32,
    window: RateLimitWindow,
    key_prefix: String,
    by: RateLimitBy,
}

impl RateLimit {
    /// Create a rate limiter allowing `max` requests per minute (default window).
    pub fn new(max: u32) -> Self {
        Self {
            max,
            window: RateLimitWindow::Minute,
            key_prefix: "rl:".to_string(),
            by: RateLimitBy::Ip,
        }
    }

    /// Use a per-second window.
    pub fn per_second(mut self) -> Self {
        self.window = RateLimitWindow::Second;
        self
    }

    /// Use a per-minute window.
    pub fn per_minute(mut self) -> Self {
        self.window = RateLimitWindow::Minute;
        self
    }

    /// Use a per-hour window.
    pub fn per_hour(mut self) -> Self {
        self.window = RateLimitWindow::Hour;
        self
    }

    /// Set a custom key prefix for the rate-limit counter.
    pub fn key_prefix(mut self, prefix: &str) -> Self {
        self.key_prefix = prefix.to_string();
        self
    }

    /// Rate-limit by authenticated actor ID instead of IP.
    pub fn by_actor(mut self) -> Self {
        self.by = RateLimitBy::Actor;
        self
    }

    /// Rate-limit by actor ID when authenticated, falling back to IP.
    pub fn by_actor_or_ip(mut self) -> Self {
        self.by = RateLimitBy::ActorOrIp;
        self
    }

    /// Returns the configured rate-limit key strategy.
    pub fn rate_limit_by(&self) -> RateLimitBy {
        self.by
    }

    /// Returns the maximum requests allowed per window.
    pub fn max(&self) -> u32 {
        self.max
    }

    /// Returns the configured window.
    pub fn window(&self) -> RateLimitWindow {
        self.window
    }

    /// Returns the key prefix.
    pub fn key_prefix_str(&self) -> &str {
        &self.key_prefix
    }

    /// Convert into a `MiddlewareConfig`.
    pub fn build(self) -> MiddlewareConfig {
        MiddlewareConfig::RateLimit(self)
    }

    fn apply(self, router: axum::Router<AppContext>, app: &AppContext) -> axum::Router<AppContext> {
        let store = create_rate_limit_store(app);
        let state = RateLimitState {
            max: self.max,
            window: self.window,
            key_prefix: self.key_prefix,
            store,
        };
        router.layer(middleware::from_fn_with_state(state, rate_limit_middleware))
    }
}

/// Create a rate-limit store backend from the application context.
///
/// Uses Redis when configured, otherwise falls back to in-memory storage.
pub(crate) fn create_rate_limit_store(app: &AppContext) -> RateLimitStore {
    match app.resolve::<RuntimeBackend>() {
        Ok(backend) if matches!(backend.kind(), RuntimeBackendKind::Redis) => {
            tracing::debug!("forge: rate limiter using Redis backend");
            RateLimitStore::Redis((*backend).clone())
        }
        _ => RateLimitStore::Memory(Arc::new(Mutex::new(HashMap::new()))),
    }
}

#[derive(Clone)]
pub(crate) struct RateLimitState {
    pub(crate) max: u32,
    pub(crate) window: RateLimitWindow,
    pub(crate) key_prefix: String,
    pub(crate) store: RateLimitStore,
}

async fn rate_limit_middleware(
    State(state): State<RateLimitState>,
    request: Request,
    next: Next,
) -> Response {
    let ip = extract_client_ip(&request);
    let key_identifier = format!("ip:{}", ip);
    let info = rate_limit_info(&state, &key_identifier).await;

    if info.current > info.limit {
        let body = serde_json::json!({
            "message": "Rate limit exceeded",
            "status": 429
        });

        return (
            StatusCode::TOO_MANY_REQUESTS,
            [
                (
                    HeaderName::from_static("x-ratelimit-limit"),
                    HeaderValue::from_str(&info.limit.to_string()).unwrap(),
                ),
                (
                    HeaderName::from_static("x-ratelimit-remaining"),
                    HeaderValue::from_str("0").unwrap(),
                ),
                (
                    HeaderName::from_static("x-ratelimit-reset"),
                    HeaderValue::from_str(&info.secs_until_reset.to_string()).unwrap(),
                ),
                (
                    header::RETRY_AFTER,
                    HeaderValue::from_str(&info.secs_until_reset.to_string()).unwrap(),
                ),
            ],
            axum::Json(body),
        )
            .into_response();
    }

    let mut response = next.run(request).await;
    let resp_headers = response.headers_mut();
    let _ = resp_headers.try_insert(
        HeaderName::from_static("x-ratelimit-limit"),
        HeaderValue::from_str(&info.limit.to_string()).unwrap(),
    );
    let _ = resp_headers.try_insert(
        HeaderName::from_static("x-ratelimit-remaining"),
        HeaderValue::from_str(&info.remaining.to_string()).unwrap(),
    );
    let _ = resp_headers.try_insert(
        HeaderName::from_static("x-ratelimit-reset"),
        HeaderValue::from_str(&info.secs_until_reset.to_string()).unwrap(),
    );
    response
}

struct RateLimitInfo {
    current: u32,
    remaining: u32,
    limit: u32,
    secs_until_reset: u64,
}

/// Increment the rate-limit counter for the given key and return current info.
async fn rate_limit_info(state: &RateLimitState, key_identifier: &str) -> RateLimitInfo {
    let window_secs = state.window.duration_secs();
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let bucket = now_secs / window_secs;
    let key = format!("{}{}:{}", state.key_prefix, key_identifier, bucket);

    let (current, secs_until_reset) = match &state.store {
        RateLimitStore::Redis(backend) => {
            let count = match backend.incr_with_ttl(&key, window_secs).await {
                Ok(c) => c as u32,
                Err(_) => {
                    tracing::warn!("forge: redis rate limit error, allowing request");
                    1
                }
            };
            let secs_until_reset = (bucket + 1) * window_secs - now_secs;
            (count, secs_until_reset)
        }
        RateLimitStore::Memory(store) => {
            let window_end_secs = (bucket + 1) * window_secs;
            let mut store = store.lock().await;
            let entry = store.entry(key).or_insert((0, window_end_secs));

            if now_secs >= entry.1 {
                *entry = (0, window_end_secs);
            }

            entry.0 += 1;
            let count = entry.0;

            if store.len() > 10_000 {
                store.retain(|_, (_, expires_at)| now_secs < *expires_at);
            }

            (count, window_end_secs.saturating_sub(now_secs))
        }
    };

    RateLimitInfo {
        current,
        remaining: state.max.saturating_sub(current),
        limit: state.max,
        secs_until_reset,
    }
}

/// Check the rate limit for a given key identifier and return a 429 response if exceeded.
///
/// Returns `Some(Response)` with a 429 status if the limit is exceeded, `None` otherwise.
/// The response includes standard rate-limit headers.
pub(crate) async fn enforce_rate_limit(state: &RateLimitState, key_identifier: &str) -> Option<Response> {
    let info = rate_limit_info(state, key_identifier).await;

    if info.current > info.limit {
        let body = serde_json::json!({
            "message": "Rate limit exceeded",
            "status": 429
        });

        return Some(
            (
                StatusCode::TOO_MANY_REQUESTS,
                [
                    (
                        HeaderName::from_static("x-ratelimit-limit"),
                        HeaderValue::from_str(&info.limit.to_string()).unwrap(),
                    ),
                    (
                        HeaderName::from_static("x-ratelimit-remaining"),
                        HeaderValue::from_str("0").unwrap(),
                    ),
                    (
                        HeaderName::from_static("x-ratelimit-reset"),
                        HeaderValue::from_str(&info.secs_until_reset.to_string()).unwrap(),
                    ),
                    (
                        header::RETRY_AFTER,
                        HeaderValue::from_str(&info.secs_until_reset.to_string()).unwrap(),
                    ),
                ],
                axum::Json(body),
            )
                .into_response(),
        );
    }

    None
}

fn extract_client_ip(request: &Request) -> IpAddr {
    // Prefer RealIp set by TrustedProxy middleware
    if let Some(RealIp(ip)) = request.extensions().get::<RealIp>() {
        return *ip;
    }
    // Fall back to connect info
    if let Some(addr) = request.extensions().get::<ConnectInfoAddr>() {
        return addr.0.ip();
    }
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}

/// Helper type used to inject a connect-info address in tests.
#[derive(Clone, Debug)]
pub(crate) struct ConnectInfoAddr(pub SocketAddr);

// ---------------------------------------------------------------------------
// MaxBodySize
// ---------------------------------------------------------------------------

/// Request body size limit middleware.
///
/// Wraps `tower_http::limit::RequestBodyLimitLayer`.
#[derive(Clone, Debug)]
pub struct MaxBodySize(usize);

impl MaxBodySize {
    /// Limit to `n` bytes.
    pub fn bytes(n: usize) -> Self {
        Self(n)
    }

    /// Limit to `n` kilobytes.
    pub fn kb(n: usize) -> Self {
        Self(n * 1024)
    }

    /// Limit to `n` megabytes.
    pub fn mb(n: usize) -> Self {
        Self(n * 1024 * 1024)
    }

    /// Convert into a `MiddlewareConfig`.
    pub fn build(self) -> MiddlewareConfig {
        MiddlewareConfig::MaxBodySize(self)
    }

    fn apply(self, router: axum::Router<AppContext>) -> axum::Router<AppContext> {
        router.layer(RequestBodyLimitLayer::new(self.0))
    }
}

// ---------------------------------------------------------------------------
// RequestTimeout
// ---------------------------------------------------------------------------

/// Request timeout middleware.
///
/// Wraps `tower_http::timeout::TimeoutLayer`.
#[derive(Clone, Debug)]
pub struct RequestTimeout(Duration);

impl RequestTimeout {
    /// Timeout after `n` seconds.
    pub fn secs(n: u64) -> Self {
        Self(Duration::from_secs(n))
    }

    /// Timeout after `n` minutes.
    pub fn mins(n: u64) -> Self {
        Self(Duration::from_secs(n * 60))
    }

    /// Timeout after the given duration.
    pub fn duration(d: Duration) -> Self {
        Self(d)
    }

    /// Convert into a `MiddlewareConfig`.
    pub fn build(self) -> MiddlewareConfig {
        MiddlewareConfig::RequestTimeout(self)
    }

    fn apply(self, router: axum::Router<AppContext>) -> axum::Router<AppContext> {
        router.layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            self.0,
        ))
    }
}

// ---------------------------------------------------------------------------
// Compression
// ---------------------------------------------------------------------------

/// Response compression middleware (gzip + brotli).
///
/// Wraps `tower_http::compression::CompressionLayer`.
#[derive(Clone, Debug)]
pub struct Compression;

impl Compression {
    pub fn new() -> Self {
        Self
    }

    pub fn build(self) -> MiddlewareConfig {
        MiddlewareConfig::Compression(self)
    }

    fn apply(self, router: axum::Router<AppContext>) -> axum::Router<AppContext> {
        router.layer(tower_http::compression::CompressionLayer::new())
    }
}

impl Default for Compression {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// MaintenanceMode
// ---------------------------------------------------------------------------

/// Maintenance mode middleware.
///
/// When active, returns `503 Service Unavailable` for all requests unless
/// a valid bypass secret is supplied via the `X-Maintenance-Bypass` header
/// or a `bypass` query parameter.
///
/// Maintenance state is stored in the runtime backend (`maintenance:active` key),
/// so it works across multiple instances in a distributed setup.
///
/// ```
/// use forge::http::middleware::MaintenanceMode;
///
/// let mw = MaintenanceMode::new()
///     .bypass_secret("my-secret-token")
///     .build();
/// ```
#[derive(Clone, Debug)]
pub struct MaintenanceMode {
    bypass_secret: Option<String>,
}

impl MaintenanceMode {
    pub fn new() -> Self {
        Self {
            bypass_secret: None,
        }
    }

    /// Set a secret that allows bypassing maintenance mode.
    ///
    /// Requests can bypass maintenance by providing the secret via:
    /// - `X-Maintenance-Bypass` header
    /// - `bypass` query parameter
    pub fn bypass_secret(mut self, secret: impl Into<String>) -> Self {
        self.bypass_secret = Some(secret.into());
        self
    }

    /// Convert into a `MiddlewareConfig`.
    pub fn build(self) -> MiddlewareConfig {
        MiddlewareConfig::MaintenanceMode(self)
    }

    fn apply(self, router: axum::Router<AppContext>, app: &AppContext) -> axum::Router<AppContext> {
        router.layer(middleware::from_fn_with_state(
            MaintenanceState {
                app: app.clone(),
                bypass_secret: self.bypass_secret,
            },
            maintenance_middleware,
        ))
    }
}

impl Default for MaintenanceMode {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
struct MaintenanceState {
    app: AppContext,
    bypass_secret: Option<String>,
}

async fn maintenance_middleware(
    State(state): State<MaintenanceState>,
    request: Request,
    next: Next,
) -> Response {
    // Check if maintenance mode is active and get the stored bypass secret
    let stored_secret = match state.app.resolve::<crate::support::runtime::RuntimeBackend>() {
        Ok(backend) => backend
            .get_value("maintenance:active")
            .await
            .unwrap_or(None),
        Err(_) => None,
    };

    // Not in maintenance mode
    if stored_secret.is_none() {
        return next.run(request).await;
    }

    // Resolve bypass secret: prefer the value stored by `forge down --secret=...`,
    // fall back to the middleware-configured secret
    let bypass_secret = stored_secret
        .as_deref()
        .filter(|s| !s.is_empty())
        .or(state.bypass_secret.as_deref());

    if let Some(secret) = bypass_secret {
        // Check X-Maintenance-Bypass header
        if let Some(header_value) = request.headers().get("x-maintenance-bypass") {
            if let Ok(value) = header_value.to_str() {
                if crate::support::hmac::constant_time_eq(value.as_bytes(), secret.as_bytes()) {
                    return next.run(request).await;
                }
            }
        }

        // Check bypass query parameter
        if let Some(query) = request.uri().query() {
            for param in query.split('&') {
                if let Some(value) = param.strip_prefix("bypass=") {
                    if crate::support::hmac::constant_time_eq(value.as_bytes(), secret.as_bytes())
                    {
                        return next.run(request).await;
                    }
                }
            }
        }
    }

    // Return 503 Service Unavailable
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({
            "message": "Service is undergoing maintenance",
            "status": 503,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// TrustedProxy
// ---------------------------------------------------------------------------

const CF_CONNECTING_IP: &str = "cf-connecting-ip";
const X_REAL_IP: &str = "x-real-ip";
const X_FORWARDED_FOR: &str = "x-forwarded-for";

/// Trusted proxy middleware.
///
/// Resolves the real client IP from proxy headers. Headers are checked in
/// priority order:
/// 1. `CF-Connecting-IP` (Cloudflare)
/// 2. `X-Real-IP` (nginx)
/// 3. `X-Forwarded-For` (first entry)
/// 4. Any custom headers registered via `with_header()`
///
/// The resolved IP is stored as a [`RealIp`] extension.
#[derive(Clone, Debug)]
pub struct TrustedProxy {
    custom_headers: Vec<HeaderName>,
}

impl TrustedProxy {
    /// Create with default header priority (CF-Connecting-IP, X-Real-IP, X-Forwarded-For).
    pub fn new() -> Self {
        Self {
            custom_headers: Vec::new(),
        }
    }

    /// Alias for `new()` — documents Cloudflare support.
    pub fn cloudflare() -> Self {
        Self::new()
    }

    /// Append a custom header to the priority list (checked after the defaults).
    pub fn with_header(mut self, hdr: HeaderName) -> Self {
        self.custom_headers.push(hdr);
        self
    }

    /// Convert into a `MiddlewareConfig`.
    pub fn build(self) -> MiddlewareConfig {
        MiddlewareConfig::TrustedProxy(self)
    }

    fn apply(self, router: axum::Router<AppContext>) -> axum::Router<AppContext> {
        let custom_headers = self.custom_headers;
        router.layer(middleware::from_fn(move |request: Request, next: Next| {
            let custom_headers = custom_headers.clone();
            async move { trusted_proxy_fn(request, next, &custom_headers).await }
        }))
    }
}

impl Default for TrustedProxy {
    fn default() -> Self {
        Self::new()
    }
}

async fn trusted_proxy_fn(
    mut request: Request,
    next: Next,
    custom_headers: &[HeaderName],
) -> Response {
    let ip = resolve_real_ip(request.headers(), custom_headers);
    request.extensions_mut().insert(RealIp(ip));
    next.run(request).await
}

pub(crate) fn resolve_real_ip(headers: &HeaderMap, custom_headers: &[HeaderName]) -> IpAddr {
    // 1. CF-Connecting-IP
    if let Some(ip) = headers
        .get(CF_CONNECTING_IP)
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .and_then(|s: &str| s.trim().parse::<IpAddr>().ok())
    {
        return ip;
    }

    // 2. X-Real-IP
    if let Some(ip) = headers
        .get(X_REAL_IP)
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .and_then(|s: &str| s.trim().parse::<IpAddr>().ok())
    {
        return ip;
    }

    // 3. X-Forwarded-For (first entry)
    if let Some(ip) = headers
        .get(X_FORWARDED_FOR)
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .and_then(|s: &str| s.split(',').next())
        .and_then(|s: &str| s.trim().parse::<IpAddr>().ok())
    {
        return ip;
    }

    // 4. Custom headers
    for header_name in custom_headers {
        if let Some(ip) = headers
            .get(header_name)
            .and_then(|v: &HeaderValue| v.to_str().ok())
            .and_then(|s: &str| s.trim().parse::<IpAddr>().ok())
        {
            return ip;
        }
    }

    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}

// ---------------------------------------------------------------------------
// ETag — Conditional response middleware
// ---------------------------------------------------------------------------

/// ETag / conditional response middleware.
///
/// Computes a SHA-256 based ETag for successful responses and returns
/// `304 Not Modified` when the client sends a matching `If-None-Match` header.
///
/// ```
/// use forge::http::middleware::ETag;
///
/// let etag = ETag::new().build();
/// ```
#[derive(Clone, Debug)]
pub struct ETag;

impl ETag {
    pub fn new() -> Self {
        Self
    }

    /// Convert into a `MiddlewareConfig`.
    pub fn build(self) -> MiddlewareConfig {
        MiddlewareConfig::ETag(self)
    }

    fn apply(self, router: axum::Router<AppContext>) -> axum::Router<AppContext> {
        router.layer(middleware::from_fn(etag_middleware))
    }
}

impl Default for ETag {
    fn default() -> Self {
        Self::new()
    }
}

async fn etag_middleware(request: Request, next: Next) -> Response {
    let if_none_match = request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let response = next.run(request).await;

    // Only compute ETag for successful responses
    if !response.status().is_success() {
        return response;
    }

    // Skip ETag for large responses (> 10 MB) to avoid excessive memory use
    const ETAG_MAX_BODY_SIZE: usize = 10 * 1024 * 1024;
    let (parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, ETAG_MAX_BODY_SIZE).await {
        Ok(bytes) => bytes,
        Err(_) => return Response::from_parts(parts, axum::body::Body::empty()),
    };

    // Compute ETag from body hash (truncated to 32 hex chars for compactness)
    let hash = crate::support::sha256_hex(&bytes);
    let etag = format!("\"{}\"", &hash[..32]);

    // Check If-None-Match
    if let Some(ref client_etag) = if_none_match {
        let trimmed = client_etag.trim();
        if trimmed == etag || trimmed.trim_matches('"') == &hash[..32] {
            return Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, &etag)
                .body(axum::body::Body::empty())
                .unwrap();
        }
    }

    // Build response with ETag header
    let mut response = Response::from_parts(parts, axum::body::Body::from(bytes));
    if let Ok(etag_value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, etag_value);
    }
    response
}

// ---------------------------------------------------------------------------
// MiddlewareGroups — named groups for reuse on routes
// ---------------------------------------------------------------------------

/// Named middleware groups registered on `AppBuilder`.
#[derive(Clone, Debug, Default)]
pub struct MiddlewareGroups(pub std::collections::HashMap<String, Vec<MiddlewareConfig>>);

impl MiddlewareGroups {
    pub fn get(&self, name: &str) -> Option<&Vec<MiddlewareConfig>> {
        self.0.get(name)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use tower::ServiceExt;

    async fn ok_handler() -> &'static str {
        "ok"
    }

    // ---- Cors tests ----

    #[tokio::test]
    async fn cors_preflight_returns_correct_headers() {
        let cors = Cors::new()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header();

        let router = axum::Router::<()>::new()
            .route("/", get(ok_handler))
            .layer(build_cors_layer(cors));

        let request = HttpRequest::builder()
            .method("OPTIONS")
            .header(header::ORIGIN, "https://example.com")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        // CORS layer forwards to the handler; the handler returns 200 with "ok"
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "*"
        );
    }

    #[tokio::test]
    async fn cors_actual_request_with_origin() {
        let cors = Cors::new()
            .allow_origin("https://example.com")
            .allow_any_method()
            .allow_any_header();

        let router = axum::Router::<()>::new()
            .route("/", get(ok_handler))
            .layer(build_cors_layer(cors));

        let request = HttpRequest::builder()
            .header(header::ORIGIN, "https://example.com")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "https://example.com"
        );
    }

    fn build_cors_layer(cors: Cors) -> CorsLayer {
        let mut layer = CorsLayer::new();
        layer = match cors.origins {
            CorsOrigins::Any => layer.allow_origin(tower_http::cors::Any),
            CorsOrigins::List(ref origins) if origins.len() == 1 => {
                let v = HeaderValue::from_str(&origins[0]).unwrap();
                layer.allow_origin(v)
            }
            CorsOrigins::List(ref origins) => {
                let values: Vec<HeaderValue> = origins
                    .iter()
                    .filter_map(|o| HeaderValue::from_str(o).ok())
                    .collect();
                layer.allow_origin(values)
            }
            CorsOrigins::None => layer,
        };
        layer = match cors.methods {
            CorsMethods::Any => layer.allow_methods(tower_http::cors::Any),
            CorsMethods::List(methods) => layer.allow_methods(methods),
            CorsMethods::None => layer,
        };
        layer = match cors.headers {
            CorsHeaders::Any => layer.allow_headers(tower_http::cors::Any),
            CorsHeaders::List(headers) => layer.allow_headers(headers),
            CorsHeaders::None => layer,
        };
        layer
    }

    // ---- SecurityHeaders tests ----

    #[tokio::test]
    async fn security_headers_adds_defaults() {
        let config = SecurityHeaders::new();
        let headers_vec = config.headers.clone();

        let router =
            axum::Router::<()>::new()
                .route("/", get(ok_handler))
                .layer(axum::middleware::from_fn(
                    move |req: Request, next: Next| {
                        let h = headers_vec.clone();
                        async move {
                            let mut resp: Response = next.run(req).await;
                            for (name, value) in &h {
                                resp.headers_mut().insert(name.clone(), value.clone());
                            }
                            resp
                        }
                    },
                ));

        let request = HttpRequest::builder().body(Body::empty()).unwrap();
        let response = router.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff"
        );
        assert_eq!(response.headers().get(X_FRAME_OPTIONS).unwrap(), "DENY");
        assert!(response.headers().get(HSTS_HEADER).is_some());
        assert!(response.headers().get(REFERRER_POLICY).is_some());
        assert_eq!(response.headers().get(X_XSS_PROTECTION).unwrap(), "0");
    }

    #[tokio::test]
    async fn security_headers_disable_hsts() {
        let config = SecurityHeaders::new().disable_hsts();
        assert!(!config.headers.iter().any(|(n, _)| *n == HSTS_HEADER));
    }

    #[tokio::test]
    async fn security_headers_custom_frame_options() {
        let config = SecurityHeaders::new().frame_options("SAMEORIGIN");
        let frame_entry = config.headers.iter().find(|(n, _)| *n == X_FRAME_OPTIONS);
        assert!(frame_entry.is_some());
        assert_eq!(frame_entry.unwrap().1, "SAMEORIGIN");
    }

    // ---- RateLimit tests ----

    #[tokio::test]
    async fn rate_limit_allows_under_limit() {
        let state = RateLimitState {
            max: 2,
            window: RateLimitWindow::Minute,
            key_prefix: "test:".to_string(),
            store: RateLimitStore::Memory(Arc::new(Mutex::new(HashMap::new()))),
        };

        let router = axum::Router::new().route("/", get(ok_handler)).layer(
            axum::middleware::from_fn_with_state(state.clone(), rate_limit_middleware),
        );

        let request = HttpRequest::builder().body(Body::empty()).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-ratelimit-remaining").unwrap(),
            "1"
        );
    }

    #[tokio::test]
    async fn rate_limit_blocks_over_limit() {
        let state = RateLimitState {
            max: 1,
            window: RateLimitWindow::Minute,
            key_prefix: "test:".to_string(),
            store: RateLimitStore::Memory(Arc::new(Mutex::new(HashMap::new()))),
        };

        let router = axum::Router::new().route("/", get(ok_handler)).layer(
            axum::middleware::from_fn_with_state(state.clone(), rate_limit_middleware),
        );

        // First request passes
        let request = HttpRequest::builder().body(Body::empty()).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Second request is blocked
        let request = HttpRequest::builder().body(Body::empty()).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().get(header::RETRY_AFTER).is_some());
    }

    // ---- TrustedProxy tests ----

    #[tokio::test]
    async fn trusted_proxy_x_forwarded_for() {
        let headers_to_check: Vec<HeaderName> = vec![];
        let router =
            axum::Router::<()>::new()
                .route("/", get(ok_handler))
                .layer(axum::middleware::from_fn(move |req, next| {
                    let h = headers_to_check.clone();
                    async move { trusted_proxy_fn(req, next, &h).await }
                }));

        let request = HttpRequest::builder()
            .header(X_FORWARDED_FOR, "1.2.3.4, 5.6.7.8")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn trusted_proxy_cf_connecting_ip_takes_priority() {
        let ip = resolve_real_ip(
            &HeaderMap::from_iter([
                (
                    HeaderName::from_static("cf-connecting-ip"),
                    HeaderValue::from_static("10.0.0.1"),
                ),
                (
                    HeaderName::from_static("x-real-ip"),
                    HeaderValue::from_static("10.0.0.2"),
                ),
            ]),
            &[],
        );
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[tokio::test]
    async fn trusted_proxy_x_real_ip_when_no_cf() {
        let ip = resolve_real_ip(
            &HeaderMap::from_iter([
                (
                    HeaderName::from_static("x-real-ip"),
                    HeaderValue::from_static("10.0.0.3"),
                ),
                (
                    HeaderName::from_static("x-forwarded-for"),
                    HeaderValue::from_static("10.0.0.4"),
                ),
            ]),
            &[],
        );
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)));
    }

    #[tokio::test]
    async fn trusted_proxy_custom_header() {
        let custom = HeaderName::from_static("x-custom-ip");
        let ip = resolve_real_ip(
            &HeaderMap::from_iter([(custom.clone(), HeaderValue::from_static("10.0.0.5"))]),
            &[custom],
        );
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)));
    }

    // ---- MiddlewareConfig ordering ----

    #[test]
    fn middleware_ordering_priorities() {
        let configs = [
            MiddlewareConfig::MaxBodySize(MaxBodySize::mb(1)),
            MiddlewareConfig::Cors(Cors::new()),
            MiddlewareConfig::TrustedProxy(TrustedProxy::new()),
            MiddlewareConfig::MaintenanceMode(MaintenanceMode::new()),
            MiddlewareConfig::Csrf(Csrf::new()),
            MiddlewareConfig::RateLimit(RateLimit::new(100)),
            MiddlewareConfig::RequestTimeout(RequestTimeout::secs(30)),
            MiddlewareConfig::ETag(ETag::new()),
            MiddlewareConfig::SecurityHeaders(SecurityHeaders::new()),
            MiddlewareConfig::Compression(Compression::new()),
        ];

        let mut with_priorities: Vec<(u8, &str)> = configs
            .iter()
            .map(|c| {
                let name = match c {
                    MiddlewareConfig::TrustedProxy(_) => "TrustedProxy",
                    MiddlewareConfig::MaintenanceMode(_) => "MaintenanceMode",
                    MiddlewareConfig::Cors(_) => "Cors",
                    MiddlewareConfig::SecurityHeaders(_) => "SecurityHeaders",
                    MiddlewareConfig::Csrf(_) => "Csrf",
                    MiddlewareConfig::RateLimit(_) => "RateLimit",
                    MiddlewareConfig::MaxBodySize(_) => "MaxBodySize",
                    MiddlewareConfig::RequestTimeout(_) => "RequestTimeout",
                    MiddlewareConfig::ETag(_) => "ETag",
                    MiddlewareConfig::Compression(_) => "Compression",
                };
                (c.priority(), name)
            })
            .collect();

        with_priorities.sort_by_key(|(p, _)| *p);

        let names: Vec<&str> = with_priorities.iter().map(|(_, n)| *n).collect();
        assert_eq!(
            names,
            vec![
                "TrustedProxy",
                "MaintenanceMode",
                "Cors",
                "SecurityHeaders",
                "Csrf",
                "RateLimit",
                "MaxBodySize",
                "RequestTimeout",
                "ETag",
                "Compression",
            ]
        );
    }
}
