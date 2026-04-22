use std::net::IpAddr;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::auth::Actor;
use crate::http::middleware::RealIp;

use super::RequestId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentRequest {
    pub request_id: Option<String>,
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    pub audit_area: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExecutionContext {
    Http {
        method: String,
        path: String,
        request_id: Option<String>,
    },
    Job {
        class: String,
        id: String,
    },
    Scheduler {
        id: String,
    },
    Other,
}

tokio::task_local! {
    static CURRENT_REQUEST: CurrentRequest;
}

tokio::task_local! {
    static CURRENT_ACTOR: Actor;
}

tokio::task_local! {
    static CURRENT_EXECUTION: ExecutionContext;
}

impl CurrentRequest {
    pub(crate) fn from_parts(parts: &Parts) -> Self {
        if let Some(current) = parts.extensions.get::<Self>() {
            return current.clone();
        }

        Self {
            request_id: parts
                .extensions
                .get::<RequestId>()
                .map(|value| value.as_str().to_string()),
            ip: parts.extensions.get::<RealIp>().map(|value| value.0),
            user_agent: parts
                .headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned),
            audit_area: None,
        }
    }

    pub(crate) fn with_audit_area(mut self, audit_area: Option<String>) -> Self {
        self.audit_area = audit_area;
        self
    }
}

impl<S> FromRequestParts<S> for CurrentRequest
where
    S: Send + Sync,
{
    type Rejection = crate::foundation::Error;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self::from_parts(parts))
    }
}

pub(crate) fn current_request() -> Option<CurrentRequest> {
    CURRENT_REQUEST.try_with(|request| request.clone()).ok()
}

pub(crate) fn current_actor() -> Option<Actor> {
    CURRENT_ACTOR.try_with(|actor| actor.clone()).ok()
}

pub(crate) fn current_execution() -> Option<ExecutionContext> {
    CURRENT_EXECUTION.try_with(|context| context.clone()).ok()
}

pub(crate) async fn scope_current_request<F, T>(request: CurrentRequest, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_REQUEST.scope(request, future).await
}

pub(crate) async fn scope_current_actor<F, T>(actor: Actor, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_ACTOR.scope(actor, future).await
}

pub(crate) async fn scope_current_execution<F, T>(context: ExecutionContext, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_EXECUTION.scope(context, future).await
}
