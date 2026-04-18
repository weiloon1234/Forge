use std::ops::{Deref, DerefMut};

use async_trait::async_trait;
use axum::extract::{FromRef, FromRequest, Request};
use axum::response::{IntoResponse, Response};
use axum::{http::StatusCode, Json};
use serde::de::DeserializeOwned;

use crate::foundation::{AppContext, Error, Result};
use crate::validation::from_multipart::FromMultipart;
use crate::validation::validator::Validator;

#[async_trait]
pub trait RequestValidator: Send + Sync {
    async fn validate(&self, validator: &mut Validator) -> Result<()>;

    /// Custom validation messages for specific field+rule combinations.
    ///
    /// Key: `(field_name, rule_code)` -> custom message.
    /// Messages support `{{attribute}}` and rule-specific placeholders.
    fn messages(&self) -> Vec<(String, String, String)> {
        Vec::new()
    }

    /// Custom display names for fields.
    ///
    /// Key: `field_name` -> display name (used as `{{attribute}}` in messages).
    fn attributes(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

pub struct Validated<T>(pub T);
pub struct JsonValidated<T>(pub T);

impl<T> Deref for Validated<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Validated<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> Deref for JsonValidated<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for JsonValidated<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T, S> FromRequest<S> for Validated<T>
where
    T: DeserializeOwned + RequestValidator + FromMultipart + Send + Sync,
    S: Send + Sync,
    AppContext: FromRef<S>,
{
    type Rejection = Response;

    fn from_request(
        req: Request,
        state: &S,
    ) -> impl std::future::Future<Output = std::result::Result<Self, Self::Rejection>> + Send {
        let app = AppContext::from_ref(state);
        let content_type = req
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        async move {
            let is_multipart = content_type.starts_with("multipart/form-data");
            let locale = resolve_request_locale(&app, req.headers(), req.extensions());

            let value = if is_multipart {
                let mut multipart = axum::extract::Multipart::from_request(req, state)
                    .await
                    .map_err(|rejection| {
                        (StatusCode::BAD_REQUEST, rejection.body_text()).into_response()
                    })?;

                T::from_multipart(&mut multipart)
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()).into_response())?
            } else {
                let Json(v) = Json::<T>::from_request(req, state).await.map_err(|error| {
                    (StatusCode::BAD_REQUEST, error.body_text()).into_response()
                })?;
                v
            };

            validate_value(value, app, locale).await.map(Self)
        }
    }
}

impl<T, S> FromRequest<S> for JsonValidated<T>
where
    T: DeserializeOwned + RequestValidator + Send + Sync,
    S: Send + Sync,
    AppContext: FromRef<S>,
{
    type Rejection = Response;

    fn from_request(
        req: Request,
        state: &S,
    ) -> impl std::future::Future<Output = std::result::Result<Self, Self::Rejection>> + Send {
        let app = AppContext::from_ref(state);
        let locale = resolve_request_locale(&app, req.headers(), req.extensions());

        async move {
            let Json(value) = Json::<T>::from_request(req, state)
                .await
                .map_err(|error| (error.status(), error.body_text()).into_response())?;

            validate_value(value, app, locale).await.map(Self)
        }
    }
}

async fn validate_value<T>(
    value: T,
    app: AppContext,
    locale: Option<String>,
) -> std::result::Result<T, Response>
where
    T: RequestValidator + Send + Sync,
{
    let mut validator = Validator::new(app);
    if let Some(locale) = locale {
        validator.set_locale(locale);
    }

    for (field, code, msg) in value.messages() {
        validator.custom_message(field, code, msg);
    }
    for (field, name) in value.attributes() {
        validator.custom_attribute(field, name);
    }

    value
        .validate(&mut validator)
        .await
        .map_err(|error| internal_error(error).into_response())?;
    validator.finish().map_err(IntoResponse::into_response)?;

    Ok(value)
}

pub(crate) fn resolve_request_locale(
    app: &AppContext,
    headers: &axum::http::HeaderMap,
    extensions: &axum::http::Extensions,
) -> Option<String> {
    // Check Locale extension first (set by custom middleware)
    if let Some(locale) = extensions.get::<crate::i18n::Locale>() {
        return Some(locale.0.clone());
    }
    // Check Accept-Language header
    if let Ok(manager) = app.i18n() {
        if let Some(header) = headers.get("accept-language").and_then(|v| v.to_str().ok()) {
            if !header.is_empty() {
                return Some(manager.resolve_locale(header));
            }
        }
    }
    None
}

fn internal_error(error: Error) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "message": error.to_string(),
            "status": 500,
        })),
    )
}
