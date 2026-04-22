use crate::auth::Actor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CurrentRequest {
    pub(crate) request_id: String,
    pub(crate) ip: Option<String>,
    pub(crate) user_agent: Option<String>,
}

tokio::task_local! {
    static CURRENT_REQUEST: CurrentRequest;
}

tokio::task_local! {
    static CURRENT_ACTOR: Actor;
}

pub(crate) fn current_request() -> Option<CurrentRequest> {
    CURRENT_REQUEST.try_with(|request| request.clone()).ok()
}

pub(crate) fn current_actor() -> Option<Actor> {
    CURRENT_ACTOR.try_with(|actor| actor.clone()).ok()
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
