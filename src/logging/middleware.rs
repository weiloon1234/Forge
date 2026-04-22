use axum::extract::{Request, State};
use axum::http::header::HeaderName;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

use super::context::CurrentRequest;
use super::request_id::{generate_request_id, RequestId, REQUEST_ID_HEADER};
use super::scope_current_request;
use crate::foundation::AppContext;

pub(crate) async fn request_context_middleware(
    State(app): State<AppContext>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(generate_request_id);

    request
        .extensions_mut()
        .insert(RequestId::new(request_id.clone()));

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let user_agent = request
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let span = tracing::info_span!(
        "forge.http.request",
        method = %method,
        path = %path,
        request_id = %request_id
    );

    let locale = resolve_request_locale(&request, &app);
    let start = std::time::Instant::now();
    let execution_context = super::ExecutionContext::Http {
        method: method.to_string(),
        path: path.clone(),
        request_id: Some(request_id.clone()),
    };
    let mut response = super::scope_current_execution(
        execution_context,
        crate::translations::CURRENT_LOCALE.scope(locale, next.run(request).instrument(span)),
    )
    .await;
    let duration_ms = start.elapsed().as_millis() as u64;

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
    let current_request = response
        .extensions()
        .get::<CurrentRequest>()
        .cloned()
        .unwrap_or(CurrentRequest {
            request_id: Some(request_id.clone()),
            ip: None,
            user_agent,
            audit_area: None,
        });
    let error_extension = response
        .extensions()
        .get::<super::reporter::HandlerErrorResponseExtension>()
        .cloned();
    let actor = response.extensions().get::<crate::auth::Actor>().cloned();
    super::report_handler_error_response(
        &app,
        method.as_str(),
        &path,
        &current_request,
        actor,
        error_extension,
    )
    .await;
    let status = response.status();
    if let Ok(diagnostics) = app.diagnostics() {
        diagnostics.record_http_response_with_duration(status, duration_ms);
    }

    tracing::info!(
        method = %method,
        path = %path,
        status = status.as_u16(),
        duration_ms = duration_ms,
        request_id = %request_id,
        "Request completed"
    );

    response
}

pub(crate) async fn request_origin_middleware(
    State(_app): State<AppContext>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let current = CurrentRequest::from_parts(&parts);
    let request = Request::from_parts(parts, body);

    let mut response = scope_current_request(current.clone(), next.run(request)).await;
    let current = response
        .extensions()
        .get::<CurrentRequest>()
        .cloned()
        .unwrap_or(current);
    response.extensions_mut().insert(current);
    response
}

fn resolve_request_locale(request: &Request, app: &AppContext) -> String {
    if let Some(locale) = request.extensions().get::<crate::i18n::Locale>() {
        return locale.0.clone();
    }
    match app.i18n() {
        Ok(manager) => request
            .headers()
            .get("accept-language")
            .and_then(|v| v.to_str().ok())
            .map(|s| manager.resolve_locale(s))
            .unwrap_or_else(|| manager.default_locale().to_string()),
        Err(_) => "en".to_string(),
    }
}
