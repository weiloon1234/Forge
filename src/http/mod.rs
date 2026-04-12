pub mod cookie;
pub mod middleware;
pub(crate) mod spa;

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::{self as axum_middleware, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use axum::Router;

use crate::auth::{AccessScope, AuthError};
use crate::foundation::{AppContext, Error, Result};
use crate::http::middleware::MiddlewareConfig;
use crate::logging::AuthOutcome;
use crate::support::{GuardId, PermissionId};
pub use crate::validation::Validated;

pub type RouteRegistrar = Arc<dyn Fn(&mut HttpRegistrar) -> Result<()> + Send + Sync>;
pub type HttpRouter = Router<AppContext>;

#[derive(Clone, Default)]
pub struct HttpRouteOptions {
    pub access: AccessScope,
    middlewares: Vec<MiddlewareConfig>,
    pub(crate) post_auth_rate_limit: Option<middleware::RateLimit>,
    pub(crate) doc: Option<crate::openapi::RouteDoc>,
}

impl HttpRouteOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn guard<I>(mut self, guard: I) -> Self
    where
        I: Into<GuardId>,
    {
        self.access = self.access.with_guard(guard);
        self
    }

    pub fn permission<I>(mut self, permission: I) -> Self
    where
        I: Into<PermissionId>,
    {
        self.access = self.access.with_permission(permission);
        self
    }

    pub fn permissions<I, P>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PermissionId>,
    {
        self.access = self.access.with_permissions(permissions);
        self
    }

    /// Attach a middleware to this specific route.
    ///
    /// Per-route middleware runs between global middleware and auth middleware.
    /// Multiple calls append middleware in order.
    pub fn middleware(mut self, config: MiddlewareConfig) -> Self {
        self.middlewares.push(config);
        self
    }

    /// Attach a rate limiter to this route.
    ///
    /// IP-based rate limiting runs as a normal middleware layer. Actor-based or
    /// actor-or-IP rate limiting is deferred until after authentication so the
    /// actor identity is available for keying.
    pub fn rate_limit(mut self, rate_limit: middleware::RateLimit) -> Self {
        match rate_limit.rate_limit_by() {
            middleware::RateLimitBy::Ip => {
                self.middlewares.push(rate_limit.build());
            }
            _ => {
                self.post_auth_rate_limit = Some(rate_limit);
            }
        }
        self
    }

    /// Attach OpenAPI documentation to this route.
    pub fn document(mut self, doc: crate::openapi::RouteDoc) -> Self {
        self.doc = Some(doc);
        self
    }

    fn requires_auth(&self) -> bool {
        self.access.requires_auth()
    }

    fn guard_id(&self) -> Option<&GuardId> {
        self.access.guard()
    }

    fn permissions_set(&self) -> BTreeSet<PermissionId> {
        self.access.permissions()
    }
}

enum HttpRegistration {
    Route {
        path: String,
        method_router: Box<MethodRouter<AppContext>>,
        options: HttpRouteOptions,
    },
    Nest {
        path: String,
        router: HttpRouter,
    },
    Merge {
        router: HttpRouter,
    },
}

pub struct HttpRegistrar {
    registrations: Vec<HttpRegistration>,
}

impl Default for HttpRegistrar {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpRegistrar {
    pub fn new() -> Self {
        Self {
            registrations: Vec::new(),
        }
    }

    pub fn route(&mut self, path: &str, method_router: MethodRouter<AppContext>) -> &mut Self {
        self.route_with_options(path, method_router, HttpRouteOptions::default())
    }

    pub fn route_with_options(
        &mut self,
        path: &str,
        method_router: MethodRouter<AppContext>,
        options: HttpRouteOptions,
    ) -> &mut Self {
        self.registrations.push(HttpRegistration::Route {
            path: path.to_string(),
            method_router: Box::new(method_router),
            options,
        });
        self
    }

    pub fn nest(&mut self, path: &str, router: HttpRouter) -> &mut Self {
        self.registrations.push(HttpRegistration::Nest {
            path: path.to_string(),
            router,
        });
        self
    }

    pub fn merge(&mut self, router: HttpRouter) -> &mut Self {
        self.registrations.push(HttpRegistration::Merge { router });
        self
    }

    /// Create a route group under a shared path prefix.
    ///
    /// Routes registered inside the closure are nested under `prefix`.
    ///
    /// ```ignore
    /// r.group("/admin", |r| {
    ///     r.route("/dashboard", get(dashboard));  // /admin/dashboard
    ///     r.route("/settings", get(settings));     // /admin/settings
    ///     Ok(())
    /// })?;
    /// ```
    pub fn group(
        &mut self,
        prefix: &str,
        f: impl FnOnce(&mut HttpRegistrar) -> Result<()>,
    ) -> Result<&mut Self> {
        let mut sub = HttpRegistrar::new();
        f(&mut sub)?;
        for registration in sub.registrations {
            match registration {
                HttpRegistration::Route {
                    path,
                    method_router,
                    options,
                } => {
                    self.registrations.push(HttpRegistration::Route {
                        path: format!("{prefix}{path}"),
                        method_router,
                        options,
                    });
                }
                HttpRegistration::Nest { path, router } => {
                    self.registrations.push(HttpRegistration::Nest {
                        path: format!("{prefix}{path}"),
                        router,
                    });
                }
                HttpRegistration::Merge { router } => {
                    // Merged routers cannot be trivially prefixed, so nest them.
                    self.registrations.push(HttpRegistration::Nest {
                        path: prefix.to_string(),
                        router,
                    });
                }
            }
        }
        Ok(self)
    }

    /// Create an API version group.
    ///
    /// Routes registered inside the closure are nested under `/api/v{version}`.
    ///
    /// ```ignore
    /// r.api_version(1, |r| {
    ///     r.route("/users", get(list_users));   // /api/v1/users
    ///     r.route("/orders", get(list_orders));  // /api/v1/orders
    ///     Ok(())
    /// })?;
    /// ```
    pub fn api_version(
        &mut self,
        version: u32,
        f: impl FnOnce(&mut HttpRegistrar) -> Result<()>,
    ) -> Result<&mut Self> {
        self.group(&format!("/api/v{version}"), f)
    }

    /// Collect documented routes for OpenAPI spec generation.
    pub(crate) fn collect_documented_routes(
        &self,
    ) -> Vec<crate::openapi::spec::DocumentedRoute> {
        let mut docs = Vec::new();
        for registration in &self.registrations {
            if let HttpRegistration::Route { path, options, .. } = registration {
                if let Some(ref doc) = options.doc {
                    docs.push(crate::openapi::spec::DocumentedRoute {
                        method: doc.method.clone().unwrap_or_else(|| "get".into()),
                        path: path.clone(),
                        doc: doc.clone(),
                    });
                }
            }
        }
        docs
    }

    pub fn into_router(self, app: AppContext) -> Router {
        self.into_router_with_middlewares(app, Vec::new())
    }

    pub fn into_router_with_middlewares(
        self,
        app: AppContext,
        middlewares: Vec<middleware::MiddlewareConfig>,
    ) -> Router {
        let mut router = Router::<AppContext>::new();

        for registration in self.registrations {
            match registration {
                HttpRegistration::Route {
                    path,
                    method_router,
                    options,
                } => {
                    let method_router = *method_router;
                    let route_middlewares = options.middlewares.clone();
                    let method_router = if options.requires_auth() {
                        let post_auth_rl = options.post_auth_rate_limit.as_ref().map(|rl| {
                            middleware::RateLimitState {
                                max: rl.max(),
                                window: rl.window(),
                                key_prefix: rl.key_prefix_str().to_string(),
                                store: middleware::create_rate_limit_store(&app),
                            }
                        });
                        method_router.route_layer(axum_middleware::from_fn_with_state(
                            HttpAuthState {
                                app: app.clone(),
                                options,
                                post_auth_rl,
                            },
                            http_auth_middleware,
                        ))
                    } else {
                        method_router
                    };

                    if route_middlewares.is_empty() {
                        router = router.route(&path, method_router);
                    } else {
                        let mini = Router::<AppContext>::new().route(&path, method_router);
                        let mini =
                            middleware::apply_ordered_middlewares(mini, route_middlewares, &app);
                        router = router.merge(mini);
                    }
                }
                HttpRegistration::Nest {
                    path,
                    router: nested,
                } => {
                    router = router.nest(&path, nested);
                }
                HttpRegistration::Merge { router: merged } => {
                    router = router.merge(merged);
                }
            }
        }

        // Apply user-registered middleware (CORS, security headers, rate limit, etc.)
        router = middleware::apply_ordered_middlewares(router, middlewares, &app);

        router
            .layer(axum_middleware::from_fn_with_state(
                app.clone(),
                crate::logging::request_context_middleware,
            ))
            .with_state(app)
    }
}

#[derive(Clone)]
struct HttpAuthState {
    app: AppContext,
    options: HttpRouteOptions,
    post_auth_rl: Option<middleware::RateLimitState>,
}

async fn http_auth_middleware(
    State(state): State<HttpAuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    let auth = match state.app.auth() {
        Ok(auth) => auth,
        Err(error) => {
            record_auth_outcome(&state.app, AuthOutcome::Error);
            return internal_error_response(error);
        }
    };
    let authorizer = match state.app.authorizer() {
        Ok(authorizer) => authorizer,
        Err(error) => {
            record_auth_outcome(&state.app, AuthOutcome::Error);
            return internal_error_response(error);
        }
    };
    let actor = match auth
        .authenticate_headers(request.headers(), state.options.guard_id())
        .await
    {
        Ok(actor) => actor,
        Err(error) => {
            record_auth_outcome(&state.app, auth_outcome_from_error(&error));
            return error.into_response();
        }
    };

    let permissions = state.options.permissions_set();
    if let Err(error) = authorizer.authorize_permissions(&actor, &permissions).await {
        record_auth_outcome(&state.app, auth_outcome_from_error(&error));
        return error.into_response();
    }

    // Post-auth rate limiting (for by_actor / by_actor_or_ip)
    if let Some(ref rl_state) = state.post_auth_rl {
        let key_id = format!("actor:{}", actor.id);
        if let Some(rejection) = middleware::enforce_rate_limit(rl_state, &key_id).await {
            return rejection;
        }
    }

    record_auth_outcome(&state.app, AuthOutcome::Success);
    request.extensions_mut().insert(state.app.clone());
    request.extensions_mut().insert(actor);
    next.run(request).await
}

fn internal_error_response(error: Error) -> Response {
    AuthError::internal(error.to_string()).into_response()
}

fn auth_outcome_from_error(error: &AuthError) -> AuthOutcome {
    match error {
        AuthError::Unauthorized(_) => AuthOutcome::Unauthorized,
        AuthError::Forbidden(_) => AuthOutcome::Forbidden,
        AuthError::Internal(_) => AuthOutcome::Error,
    }
}

fn record_auth_outcome(app: &AppContext, outcome: AuthOutcome) {
    if let Ok(diagnostics) = app.diagnostics() {
        diagnostics.record_auth_outcome(outcome);
    }
}
