use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use axum::extract::{FromRef, FromRequest, Request};
use axum::response::{IntoResponse, Response};
use axum::{http::StatusCode, Json};
use serde::de::DeserializeOwned;
use serde::Serialize;
use validator::ValidateEmail;

use crate::foundation::{AppContext, Error, Result};
use crate::support::ValidationRuleId;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ValidationError {
    pub code: String,
    pub message: String,
}

impl ValidationError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("validation failed")]
pub struct ValidationErrors {
    pub errors: Vec<FieldError>,
}

impl ValidationErrors {
    pub fn new(errors: Vec<FieldError>) -> Self {
        Self { errors }
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }
}

impl IntoResponse for ValidationErrors {
    fn into_response(self) -> Response {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "message": "Validation failed",
                "errors": self.errors,
            })),
        )
            .into_response()
    }
}

#[derive(Clone)]
pub struct RuleContext {
    app: AppContext,
    field: String,
}

impl RuleContext {
    pub fn new(app: AppContext, field: impl Into<String>) -> Self {
        Self {
            app,
            field: field.into(),
        }
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }

    pub fn field(&self) -> &str {
        &self.field
    }
}

#[async_trait]
pub trait ValidationRule: Send + Sync + 'static {
    async fn validate(
        &self,
        context: &RuleContext,
        value: &str,
    ) -> std::result::Result<(), ValidationError>;
}

#[derive(Clone, Default)]
pub struct RuleRegistry {
    rules: Arc<RwLock<HashMap<ValidationRuleId, Arc<dyn ValidationRule>>>>,
}

impl RuleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<I>(&self, id: I, rule: impl ValidationRule) -> Result<()>
    where
        I: Into<ValidationRuleId>,
    {
        self.register_arc(id, Arc::new(rule))
    }

    pub fn register_arc<I>(&self, id: I, rule: Arc<dyn ValidationRule>) -> Result<()>
    where
        I: Into<ValidationRuleId>,
    {
        let id = id.into();
        let mut rules = self
            .rules
            .write()
            .map_err(|_| Error::message("rule registry lock poisoned"))?;
        if rules.contains_key(&id) {
            return Err(Error::message(format!(
                "validation rule `{id}` already registered"
            )));
        }
        rules.insert(id, rule);
        Ok(())
    }

    pub fn get(&self, id: &ValidationRuleId) -> Result<Option<Arc<dyn ValidationRule>>> {
        Ok(self
            .rules
            .read()
            .map_err(|_| Error::message("rule registry lock poisoned"))?
            .get(id)
            .cloned())
    }
}

pub struct Validator {
    app: AppContext,
    errors: Vec<FieldError>,
}

impl Validator {
    pub fn new(app: AppContext) -> Self {
        Self {
            app,
            errors: Vec::new(),
        }
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }

    pub fn field<'a>(
        &'a mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> FieldValidator<'a> {
        FieldValidator {
            validator: self,
            field: name.into(),
            value: value.into(),
            steps: Vec::new(),
        }
    }

    pub fn finish(self) -> std::result::Result<(), ValidationErrors> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors::new(self.errors))
        }
    }

    fn push_error(&mut self, field: String, error: ValidationError) {
        self.errors.push(FieldError {
            field,
            code: error.code,
            message: error.message,
        });
    }
}

enum FieldRule {
    Required,
    Email,
    Min(usize),
    Max(usize),
    Named(ValidationRuleId),
}

pub struct FieldValidator<'a> {
    validator: &'a mut Validator,
    field: String,
    value: String,
    steps: Vec<FieldRule>,
}

impl<'a> FieldValidator<'a> {
    pub fn required(mut self) -> Self {
        self.steps.push(FieldRule::Required);
        self
    }

    pub fn email(mut self) -> Self {
        self.steps.push(FieldRule::Email);
        self
    }

    pub fn min(mut self, length: usize) -> Self {
        self.steps.push(FieldRule::Min(length));
        self
    }

    pub fn max(mut self, length: usize) -> Self {
        self.steps.push(FieldRule::Max(length));
        self
    }

    pub fn rule<I>(mut self, id: I) -> Self
    where
        I: Into<ValidationRuleId>,
    {
        self.steps.push(FieldRule::Named(id.into()));
        self
    }

    pub async fn apply(self) -> Result<()> {
        let FieldValidator {
            validator,
            field,
            value,
            steps,
        } = self;

        for step in steps {
            match step {
                FieldRule::Required => {
                    if value.trim().is_empty() {
                        validator.push_error(
                            field.clone(),
                            ValidationError::new("required", format!("{field} is required")),
                        );
                    }
                }
                FieldRule::Email => {
                    if !value.validate_email() {
                        validator.push_error(
                            field.clone(),
                            ValidationError::new("email", format!("{field} must be a valid email")),
                        );
                    }
                }
                FieldRule::Min(length) => {
                    if value.chars().count() < length {
                        validator.push_error(
                            field.clone(),
                            ValidationError::new(
                                "min",
                                format!("{field} must be at least {length} characters"),
                            ),
                        );
                    }
                }
                FieldRule::Max(length) => {
                    if value.chars().count() > length {
                        validator.push_error(
                            field.clone(),
                            ValidationError::new(
                                "max",
                                format!("{field} must be at most {length} characters"),
                            ),
                        );
                    }
                }
                FieldRule::Named(id) => {
                    let Some(rule) = validator.app.rules().get(&id)? else {
                        return Err(Error::message(format!(
                            "validation rule `{id}` is not registered"
                        )));
                    };
                    let context = RuleContext::new(validator.app.clone(), field.clone());
                    if let Err(error) = rule.validate(&context, &value).await {
                        validator.push_error(field.clone(), error);
                    }
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
pub trait RequestValidator: Send + Sync {
    async fn validate(&self, validator: &mut Validator) -> Result<()>;
}

pub struct Validated<T>(pub T);

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

impl<T, S> FromRequest<S> for Validated<T>
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
        async move {
            let Json(value) = Json::<T>::from_request(req, state)
                .await
                .map_err(|error| (StatusCode::BAD_REQUEST, error.body_text()).into_response())?;

            let mut validator = Validator::new(app);
            value
                .validate(&mut validator)
                .await
                .map_err(|error| internal_error(error).into_response())?;
            validator.finish().map_err(IntoResponse::into_response)?;

            Ok(Self(value))
        }
    }
}

fn internal_error(error: Error) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "message": error.to_string(),
        })),
    )
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::{RuleContext, RuleRegistry, ValidationError, ValidationRule, Validator};
    use crate::foundation::AppContext;
    use crate::support::ValidationRuleId;
    use crate::{config::ConfigRepository, foundation::Container};

    struct MobileRule;

    #[async_trait]
    impl ValidationRule for MobileRule {
        async fn validate(
            &self,
            _context: &RuleContext,
            value: &str,
        ) -> std::result::Result<(), ValidationError> {
            if value.starts_with('+') && value[1..].chars().all(|ch| ch.is_ascii_digit()) {
                Ok(())
            } else {
                Err(ValidationError::new("mobile", "invalid mobile number"))
            }
        }
    }

    #[tokio::test]
    async fn executes_custom_rules() {
        let rules = RuleRegistry::new();
        rules
            .register(ValidationRuleId::new("mobile"), MobileRule)
            .unwrap();
        let app = AppContext::new(Container::new(), ConfigRepository::empty(), rules);
        let mut validator = Validator::new(app);

        validator
            .field("phone", "123")
            .required()
            .rule(ValidationRuleId::new("mobile"))
            .apply()
            .await
            .unwrap();

        let errors = validator.finish().unwrap_err();
        assert_eq!(errors.errors.len(), 1);
        assert_eq!(errors.errors[0].code, "mobile");
    }

    #[test]
    fn rejects_duplicate_named_rules() {
        let rules = RuleRegistry::new();
        rules
            .register(ValidationRuleId::new("mobile"), MobileRule)
            .unwrap();

        let error = rules
            .register(ValidationRuleId::new("mobile"), MobileRule)
            .unwrap_err();
        assert!(error.to_string().contains("already registered"));
    }
}
