use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use forge::prelude::*;
use serde_json::Value;

mod app {
    use super::*;

    pub mod ids {
        use super::*;

        #[derive(Clone, Copy)]
        pub enum AuthGuard {
            Admin,
        }

        impl From<AuthGuard> for GuardId {
            fn from(value: AuthGuard) -> Self {
                match value {
                    AuthGuard::Admin => GuardId::new("admin"),
                }
            }
        }

        #[derive(Clone, Copy)]
        pub enum PolicyKey {
            DeveloperOnly,
        }

        impl From<PolicyKey> for PolicyId {
            fn from(value: PolicyKey) -> Self {
                match value {
                    PolicyKey::DeveloperOnly => PolicyId::new("developer_only"),
                }
            }
        }

        #[derive(Clone, Copy)]
        pub enum RoleKey {
            Developer,
        }

        impl From<RoleKey> for RoleId {
            fn from(value: RoleKey) -> Self {
                match value {
                    RoleKey::Developer => RoleId::new("developer"),
                }
            }
        }

        #[derive(Clone, Copy)]
        pub enum Ability {
            ReportsView,
            ObservabilityView,
        }

        impl From<Ability> for PermissionId {
            fn from(value: Ability) -> Self {
                match value {
                    Ability::ReportsView => PermissionId::new("reports:view"),
                    Ability::ObservabilityView => PermissionId::new("observability:view"),
                }
            }
        }
    }

    pub mod providers {
        use super::*;

        #[derive(Clone)]
        pub struct AppServiceProvider;

        pub struct DeveloperOnlyPolicy;

        #[async_trait]
        impl Policy for DeveloperOnlyPolicy {
            async fn evaluate(&self, actor: &Actor, _app: &AppContext) -> Result<bool> {
                Ok(actor.has_role(ids::RoleKey::Developer))
            }
        }

        #[async_trait]
        impl ServiceProvider for AppServiceProvider {
            async fn register(&self, registrar: &mut ServiceRegistrar) -> Result<()> {
                registrar.register_guard(
                    ids::AuthGuard::Admin,
                    StaticBearerAuthenticator::new()
                        .token("guest-token", Actor::new("guest-1", ids::AuthGuard::Admin))
                        .token(
                            "ops-token",
                            Actor::new("ops-1", ids::AuthGuard::Admin).with_permissions([
                                ids::Ability::ReportsView,
                                ids::Ability::ObservabilityView,
                            ]),
                        )
                        .token(
                            "developer-token",
                            Actor::new("developer-1", ids::AuthGuard::Admin)
                                .with_roles([ids::RoleKey::Developer])
                                .with_permissions([
                                    ids::Ability::ReportsView,
                                    ids::Ability::ObservabilityView,
                                ]),
                        ),
                )?;
                registrar.register_policy(ids::PolicyKey::DeveloperOnly, DeveloperOnlyPolicy)?;
                Ok(())
            }
        }
    }
}

fn reports_routes(
    counter: Arc<AtomicUsize>,
) -> impl Fn(&mut HttpRegistrar) -> Result<()> + Send + Sync + 'static {
    move |registrar| {
        let counter = counter.clone();
        registrar.route_with_options(
            "/reports",
            get(reports),
            HttpRouteOptions::new()
                .guard(app::ids::AuthGuard::Admin)
                .permission(app::ids::Ability::ReportsView)
                .authorize(move |ctx| {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        let allowed = ctx
                            .app()
                            .authorizer()?
                            .allows_policy(ctx.actor(), app::ids::PolicyKey::DeveloperOnly)
                            .await?;

                        if allowed {
                            Ok(())
                        } else {
                            Err(Error::http(403, "Forbidden by project policy"))
                        }
                    }
                }),
        );
        Ok(())
    }
}

fn always_unauthorized_routes() -> impl Fn(&mut HttpRegistrar) -> Result<()> + Send + Sync + 'static
{
    move |registrar| {
        registrar.route_with_options(
            "/session-check",
            get(session_check),
            HttpRouteOptions::new()
                .guard(app::ids::AuthGuard::Admin)
                .permission(app::ids::Ability::ReportsView)
                .authorize(|_ctx| async {
                    Err(AuthError::unauthorized("Re-authentication required").into())
                }),
        );
        Ok(())
    }
}

async fn reports(actor: CurrentActor) -> impl IntoResponse {
    Json(serde_json::json!({
        "actor_id": actor.id,
    }))
}

async fn session_check(actor: CurrentActor) -> impl IntoResponse {
    Json(serde_json::json!({
        "actor_id": actor.id,
    }))
}

#[tokio::test]
async fn http_route_authorizer_runs_after_permissions_and_can_return_forbidden() {
    let counter = Arc::new(AtomicUsize::new(0));
    let app = TestApp::builder()
        .register_provider(app::providers::AppServiceProvider)
        .register_routes(reports_routes(counter.clone()))
        .build()
        .await;

    let unauthorized = app.client().get("/reports").send().await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    let forbidden_by_permission = app
        .client()
        .get("/reports")
        .bearer_auth("guest-token")
        .send()
        .await;
    assert_eq!(forbidden_by_permission.status(), StatusCode::FORBIDDEN);
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    let forbidden_by_authorizer = app
        .client()
        .get("/reports")
        .bearer_auth("ops-token")
        .send()
        .await;
    assert_eq!(forbidden_by_authorizer.status(), StatusCode::FORBIDDEN);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let allowed = app
        .client()
        .get("/reports")
        .bearer_auth("developer-token")
        .send()
        .await;
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    let payload: Value = allowed.json();
    assert_eq!(payload["actor_id"], "developer-1");
}

#[tokio::test]
async fn http_route_authorizer_can_return_unauthorized() {
    let app = TestApp::builder()
        .register_provider(app::providers::AppServiceProvider)
        .register_routes(always_unauthorized_routes())
        .build()
        .await;

    let response = app
        .client()
        .get("/session-check")
        .bearer_auth("developer-token")
        .send()
        .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let payload: Value = response.json();
    assert_eq!(payload["message"], "Re-authentication required");
}

#[tokio::test]
async fn observability_authorizer_applies_to_all_routes_and_can_hide_with_not_found() {
    const OBSERVABILITY_ROUTES: [&str; 12] = [
        "/_forge/health",
        "/_forge/ready",
        "/_forge/runtime",
        "/_forge/metrics",
        "/_forge/jobs/stats",
        "/_forge/jobs/failed",
        "/_forge/sql",
        "/_forge/ws/channels",
        "/_forge/ws/stats",
        "/_forge/ws/presence/team",
        "/_forge/ws/history/team",
        "/_forge/openapi.json",
    ];

    let counter = Arc::new(AtomicUsize::new(0));
    let authorize_counter = counter.clone();
    let app = TestApp::builder()
        .register_provider(app::providers::AppServiceProvider)
        .enable_observability_with(
            ObservabilityOptions::new()
                .guard(app::ids::AuthGuard::Admin)
                .permission(app::ids::Ability::ObservabilityView)
                .authorize(move |ctx| {
                    let counter = authorize_counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        let allowed = ctx
                            .app()
                            .authorizer()?
                            .allows_policy(ctx.actor(), app::ids::PolicyKey::DeveloperOnly)
                            .await?;

                        if allowed {
                            Ok(())
                        } else {
                            Err(Error::not_found("Not found"))
                        }
                    }
                }),
        )
        .build()
        .await;

    let unauthorized = app.client().get("/_forge/health").send().await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    for path in OBSERVABILITY_ROUTES {
        let response = app
            .client()
            .get(path)
            .bearer_auth("guest-token")
            .send()
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "route {path}");
    }
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    for path in OBSERVABILITY_ROUTES {
        let response = app.client().get(path).bearer_auth("ops-token").send().await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "route {path}");
    }
    assert_eq!(counter.load(Ordering::SeqCst), OBSERVABILITY_ROUTES.len());

    let allowed = app
        .client()
        .get("/_forge/health")
        .bearer_auth("developer-token")
        .send()
        .await;
    assert_eq!(allowed.status(), StatusCode::OK);
}
