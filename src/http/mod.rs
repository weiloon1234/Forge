pub mod cookie;
pub mod middleware;
pub mod resource;
pub mod response;
pub mod routes;
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
pub use crate::validation::{JsonValidated, Validated};

pub type RouteRegistrar = Arc<dyn Fn(&mut HttpRegistrar) -> Result<()> + Send + Sync>;
pub type HttpRouter = Router<AppContext>;

#[derive(Clone, Default)]
pub struct HttpRouteOptions {
    pub access: AccessScope,
    middlewares: Vec<MiddlewareConfig>,
    middleware_group_name: Option<String>,
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

    /// Apply a named middleware group to this route.
    ///
    /// The group must have been registered via `AppBuilder::middleware_group()`.
    /// Group middlewares are prepended before any per-route middlewares.
    pub fn middleware_group(mut self, name: impl Into<String>) -> Self {
        self.middleware_group_name = Some(name.into());
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

    /// Add an OpenAPI tag without building a full [`crate::openapi::RouteDoc`] manually.
    pub fn tag(mut self, tag: &str) -> Self {
        let doc = self.doc.take().unwrap_or_default().tag(tag);
        self.doc = Some(doc);
        self
    }

    /// Add an OpenAPI summary without building a full [`crate::openapi::RouteDoc`] manually.
    pub fn summary(mut self, summary: &str) -> Self {
        let doc = self.doc.take().unwrap_or_default().summary(summary);
        self.doc = Some(doc);
        self
    }

    /// Add an OpenAPI description without building a full [`crate::openapi::RouteDoc`] manually.
    pub fn description(mut self, description: &str) -> Self {
        let doc = self.doc.take().unwrap_or_default().description(description);
        self.doc = Some(doc);
        self
    }

    pub fn request<T: crate::openapi::ApiSchema>(mut self) -> Self {
        let doc = self.doc.take().unwrap_or_default().request::<T>();
        self.doc = Some(doc);
        self
    }

    pub fn response<T: crate::openapi::ApiSchema>(mut self, status: u16) -> Self {
        let doc = self.doc.take().unwrap_or_default().response::<T>(status);
        self.doc = Some(doc);
        self
    }

    pub fn deprecated(mut self) -> Self {
        let doc = self.doc.take().unwrap_or_default().deprecated();
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

    fn with_defaults(mut self, defaults: &Self) -> Self {
        self.access = merge_access_scope(&self.access, &defaults.access);

        let mut middlewares = defaults.middlewares.clone();
        middlewares.extend(self.middlewares);
        self.middlewares = middlewares;

        if self.middleware_group_name.is_none() {
            self.middleware_group_name = defaults.middleware_group_name.clone();
        }
        if self.post_auth_rate_limit.is_none() {
            self.post_auth_rate_limit = defaults.post_auth_rate_limit.clone();
        }

        self.doc = match (self.doc.take(), defaults.doc.as_ref()) {
            (Some(doc), Some(default_doc)) => Some(doc.merge_defaults(default_doc)),
            (Some(doc), None) => Some(doc),
            (None, Some(default_doc)) => Some(default_doc.clone()),
            (None, None) => None,
        };

        self
    }
}

fn merge_access_scope(explicit: &AccessScope, defaults: &AccessScope) -> AccessScope {
    match (defaults, explicit) {
        (AccessScope::Public, _) => explicit.clone(),
        (AccessScope::Guarded(defaults), AccessScope::Public) => {
            AccessScope::Guarded(defaults.clone())
        }
        (AccessScope::Guarded(defaults), AccessScope::Guarded(explicit)) => {
            let mut merged = defaults.clone();
            if explicit.guard.is_some() {
                merged.guard = explicit.guard.clone();
            }
            merged.permissions.extend(explicit.permissions.clone());
            AccessScope::Guarded(merged)
        }
    }
}

#[derive(Default)]
pub struct HttpResourceRoutes {
    index: Option<MethodRouter<AppContext>>,
    store: Option<MethodRouter<AppContext>>,
    show: Option<MethodRouter<AppContext>>,
    update: Option<MethodRouter<AppContext>>,
    destroy: Option<MethodRouter<AppContext>>,
    id_param: String,
}

impl HttpResourceRoutes {
    pub fn new() -> Self {
        Self {
            id_param: "id".to_string(),
            ..Self::default()
        }
    }

    pub fn index(mut self, route: MethodRouter<AppContext>) -> Self {
        self.index = Some(route);
        self
    }

    pub fn store(mut self, route: MethodRouter<AppContext>) -> Self {
        self.store = Some(route);
        self
    }

    pub fn show(mut self, route: MethodRouter<AppContext>) -> Self {
        self.show = Some(route);
        self
    }

    pub fn update(mut self, route: MethodRouter<AppContext>) -> Self {
        self.update = Some(route);
        self
    }

    pub fn destroy(mut self, route: MethodRouter<AppContext>) -> Self {
        self.destroy = Some(route);
        self
    }

    pub fn id_param(mut self, id_param: impl Into<String>) -> Self {
        self.id_param = id_param.into();
        self
    }
}

struct RouteRegistration {
    path: String,
    method_router: MethodRouter<AppContext>,
    options: HttpRouteOptions,
}

enum HttpRegistration {
    Route(Box<RouteRegistration>),
    Nest { path: String, router: HttpRouter },
    Merge { router: HttpRouter },
}

pub struct HttpRegistrar {
    registrations: Vec<HttpRegistration>,
    pub(crate) named_routes: routes::RouteRegistry,
    default_route_options: HttpRouteOptions,
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
            named_routes: routes::RouteRegistry::new(),
            default_route_options: HttpRouteOptions::default(),
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
        let path_owned = path.to_string();
        self.registrations
            .push(HttpRegistration::Route(Box::new(RouteRegistration {
                path: path_owned,
                method_router,
                options: options.with_defaults(&self.default_route_options),
            })));
        self
    }

    /// Register a named route for URL generation.
    pub fn route_named(
        &mut self,
        name: &str,
        path: &str,
        method_router: MethodRouter<AppContext>,
    ) -> &mut Self {
        self.named_routes.register(name, path);
        self.route(path, method_router)
    }

    /// Register a named route with options.
    pub fn route_named_with_options(
        &mut self,
        name: &str,
        path: &str,
        method_router: MethodRouter<AppContext>,
        options: HttpRouteOptions,
    ) -> &mut Self {
        self.named_routes.register(name, path);
        self.route_with_options(path, method_router, options)
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
        sub.default_route_options = self.default_route_options.clone();
        f(&mut sub)?;
        self.merge_group(prefix, sub)
    }

    /// Create a route group under a shared path prefix with inherited defaults.
    ///
    /// Guard, middleware, rate-limit, and OpenAPI defaults from `options`
    /// apply to every route registered inside the closure.
    pub fn group_with_options(
        &mut self,
        prefix: &str,
        options: HttpRouteOptions,
        f: impl FnOnce(&mut HttpRegistrar) -> Result<()>,
    ) -> Result<&mut Self> {
        let mut sub = HttpRegistrar::new();
        sub.default_route_options = options.with_defaults(&self.default_route_options);
        f(&mut sub)?;
        self.merge_group(prefix, sub)
    }

    fn merge_group(&mut self, prefix: &str, sub: HttpRegistrar) -> Result<&mut Self> {
        for registration in sub.registrations {
            match registration {
                HttpRegistration::Route(route) => {
                    self.route_with_options(
                        &format!("{prefix}{}", route.path),
                        route.method_router,
                        route.options,
                    );
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
        // Merge named routes from sub-registrar with prefix applied
        for (name, pattern) in sub.named_routes.iter() {
            self.named_routes
                .register(name, format!("{prefix}{pattern}"));
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

    pub fn resource(&mut self, name: &str, path: &str, routes: HttpResourceRoutes) -> &mut Self {
        self.resource_with_options(name, path, routes, HttpRouteOptions::default())
    }

    pub fn resource_with_options(
        &mut self,
        name: &str,
        path: &str,
        routes: HttpResourceRoutes,
        options: HttpRouteOptions,
    ) -> &mut Self {
        if let Some(route) = routes.index {
            self.route_named_with_options(&format!("{name}.index"), path, route, options.clone());
        }
        if let Some(route) = routes.store {
            self.route_named_with_options(&format!("{name}.store"), path, route, options.clone());
        }

        let member_path = format!("{path}/:{}", routes.id_param);
        if let Some(route) = routes.show {
            self.route_named_with_options(
                &format!("{name}.show"),
                &member_path,
                route,
                options.clone(),
            );
        }
        if let Some(route) = routes.update {
            self.route_named_with_options(
                &format!("{name}.update"),
                &member_path,
                route,
                options.clone(),
            );
        }
        if let Some(route) = routes.destroy {
            self.route_named_with_options(&format!("{name}.destroy"), &member_path, route, options);
        }

        self
    }

    /// Collect documented routes for OpenAPI spec generation.
    pub(crate) fn collect_documented_routes(&self) -> Vec<crate::openapi::spec::DocumentedRoute> {
        let mut docs = Vec::new();
        for registration in &self.registrations {
            if let HttpRegistration::Route(route) = registration {
                if let Some(ref doc) = route.options.doc {
                    docs.push(crate::openapi::spec::DocumentedRoute {
                        method: doc.method.clone().unwrap_or_else(|| "get".into()),
                        path: route.path.clone(),
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
                HttpRegistration::Route(route) => {
                    let RouteRegistration {
                        path,
                        method_router,
                        options,
                    } = *route;
                    let mut route_middlewares = Vec::new();
                    // Expand middleware group if specified
                    if let Some(ref group_name) = options.middleware_group_name {
                        if let Ok(groups) = app.resolve::<middleware::MiddlewareGroups>() {
                            if let Some(group_mws) = groups.get(group_name) {
                                route_middlewares.extend(group_mws.clone());
                            }
                        }
                    }
                    route_middlewares.extend(options.middlewares.clone());
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

// ---------------------------------------------------------------------------
// Maintenance mode CLI commands (down / up)
// ---------------------------------------------------------------------------

pub(crate) fn maintenance_cli_registrar() -> crate::cli::CommandRegistrar {
    use clap::{Arg, Command};

    use crate::cli::CommandRegistrar;
    use crate::support::runtime::RuntimeBackend;
    use crate::support::CommandId;

    const DOWN_COMMAND: CommandId = CommandId::new("down");
    const UP_COMMAND: CommandId = CommandId::new("up");
    const ROUTES_LIST_COMMAND: CommandId = CommandId::new("routes:list");

    let registrar: CommandRegistrar = Arc::new(|registry| {
        registry.command(
            DOWN_COMMAND,
            Command::new(DOWN_COMMAND.as_str().to_string())
                .about("Put the application into maintenance mode")
                .arg(
                    Arg::new("secret")
                        .long("secret")
                        .value_name("SECRET")
                        .help("Bypass secret for maintenance mode"),
                ),
            |invocation| async move {
                let app = invocation.app();
                let backend = app.resolve::<RuntimeBackend>()?;
                let secret = invocation
                    .matches()
                    .get_one::<String>("secret")
                    .cloned()
                    .unwrap_or_default();

                // Clear any existing key and set fresh
                let _ = backend.del_key("maintenance:active").await;
                backend
                    .set_nx_value("maintenance:active", &secret, 31_536_000)
                    .await?;

                println!("Application is now in maintenance mode.");
                if !secret.is_empty() {
                    println!("Bypass secret: {secret}");
                }
                Ok(())
            },
        )?;

        registry.command(
            UP_COMMAND,
            Command::new(UP_COMMAND.as_str().to_string())
                .about("Bring the application out of maintenance mode"),
            |invocation| async move {
                let app = invocation.app();
                let backend = app.resolve::<RuntimeBackend>()?;
                backend.del_key("maintenance:active").await?;
                println!("Application is now live.");
                Ok(())
            },
        )?;

        registry.command(
            ROUTES_LIST_COMMAND,
            Command::new(ROUTES_LIST_COMMAND.as_str().to_string())
                .about("List all registered named routes"),
            |invocation| async move {
                let app = invocation.app();
                match app.resolve::<routes::RouteRegistry>() {
                    Ok(registry) => {
                        let mut routes: Vec<_> = registry.iter().collect();
                        routes.sort_by(|(a, _), (b, _)| a.cmp(b));
                        if routes.is_empty() {
                            println!("No named routes registered.");
                        } else {
                            println!("{:<30} PATH", "NAME");
                            println!("{}", "-".repeat(60));
                            for (name, pattern) in routes {
                                println!("{:<30} {}", name, pattern);
                            }
                        }
                    }
                    Err(_) => {
                        println!("Route registry not available (routes are built during HTTP kernel startup).");
                        println!("Named routes registered via route_named() will appear here after HTTP boot.");
                    }
                }
                Ok(())
            },
        )?;

        Ok(())
    });
    registrar
}

#[cfg(test)]
mod tests {
    use axum::routing::{delete, get, post, put};

    use super::{HttpRegistrar, HttpRegistration, HttpResourceRoutes, HttpRouteOptions};
    use crate::support::GuardId;

    async fn ok() -> &'static str {
        "ok"
    }

    #[test]
    fn group_with_options_inherits_guard_and_doc_defaults() {
        let mut registrar = HttpRegistrar::new();
        registrar
            .group_with_options(
                "/api",
                HttpRouteOptions::new()
                    .guard(GuardId::new("api"))
                    .tag("users"),
                |routes| {
                    routes.route_with_options(
                        "/users",
                        get(ok),
                        HttpRouteOptions::new().summary("List users"),
                    );
                    Ok(())
                },
            )
            .unwrap();

        let HttpRegistration::Route(route) = &registrar.registrations[0] else {
            panic!("expected route registration");
        };

        assert_eq!(route.path, "/api/users");
        assert_eq!(route.options.guard_id(), Some(&GuardId::new("api")));

        let doc = route
            .options
            .doc
            .as_ref()
            .expect("route docs should be present");
        assert_eq!(doc.tags, vec!["users".to_string()]);
        assert_eq!(doc.summary.as_deref(), Some("List users"));
    }

    #[test]
    fn resource_registers_common_named_routes() {
        let mut registrar = HttpRegistrar::new();
        registrar.resource_with_options(
            "users",
            "/users",
            HttpResourceRoutes::new()
                .index(get(ok))
                .store(post(ok))
                .show(get(ok))
                .update(put(ok))
                .destroy(delete(ok)),
            HttpRouteOptions::new().guard(GuardId::new("api")),
        );

        assert!(registrar.named_routes.has("users.index"));
        assert!(registrar.named_routes.has("users.store"));
        assert!(registrar.named_routes.has("users.show"));
        assert!(registrar.named_routes.has("users.update"));
        assert!(registrar.named_routes.has("users.destroy"));

        let registered_paths = registrar
            .registrations
            .iter()
            .filter_map(|registration| match registration {
                HttpRegistration::Route(route) => Some(route.path.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(registered_paths.contains(&"/users"));
        assert!(registered_paths.contains(&"/users/:id"));
    }
}
