use axum::extract::{Request, State};
use axum::http::header::HeaderName;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

use crate::foundation::AppContext;

use super::request_id::{generate_request_id, RequestId, REQUEST_ID_HEADER};

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

    let start = std::time::Instant::now();
    let mut response = next.run(request).instrument(span).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
    let status = response.status();
    if let Ok(diagnostics) = app.diagnostics() {
        diagnostics.record_http_response(status);
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
