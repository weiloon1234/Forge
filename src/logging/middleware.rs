use axum::extract::{Request, State};
use axum::http::header::HeaderName;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

use crate::foundation::AppContext;
use crate::http::middleware::RealIp;

use super::context::CurrentRequest;
use super::request_id::{generate_request_id, RequestId, REQUEST_ID_HEADER};
use super::scope_current_request;

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
    let span = tracing::info_span!(
        "forge.http.request",
        method = %method,
        path = %path,
        request_id = %request_id
    );

    let locale = resolve_request_locale(&request, &app);
    let start = std::time::Instant::now();
    let mut response = crate::translations::CURRENT_LOCALE
        .scope(locale, next.run(request).instrument(span))
        .await;
    let duration_ms = start.elapsed().as_millis() as u64;

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
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
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(generate_request_id);
    let current = CurrentRequest {
        request_id,
        ip: request
            .extensions()
            .get::<RealIp>()
            .map(|value| value.0.to_string()),
        user_agent: request
            .headers()
            .get(axum::http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned),
    };

    scope_current_request(current, next.run(request)).await
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
