use async_trait::async_trait;
use forge::prelude::*;

use crate::app::ids;

#[derive(Debug, Deserialize)]
pub struct CreateUser {
    pub email: String,
    pub phone: String,
}

#[async_trait]
impl RequestValidator for CreateUser {
    async fn validate(&self, validator: &mut Validator) -> Result<()> {
        validator
            .field("email", self.email.clone())
            .required()
            .email()
            .apply()
            .await?;
        validator
            .field("phone", self.phone.clone())
            .required()
            .rule(ids::MOBILE_RULE)
            .apply()
            .await?;
        Ok(())
    }
}

pub fn router(registrar: &mut HttpRegistrar) -> Result<()> {
    registrar.route("/health", get(health));
    registrar.route_with_options(
        "/users",
        post(create_user),
        HttpRouteOptions::new()
            .guard(ids::AuthGuard::Api)
            .permission(ids::Ability::DashboardView),
    );
    Ok(())
}

async fn health(State(app): State<AppContext>) -> impl IntoResponse {
    let entries = app.resolve::<std::sync::Mutex<Vec<String>>>().unwrap();
    Json(serde_json::json!({
        "entries": entries.lock().unwrap().clone(),
    }))
}

async fn create_user(_actor: CurrentActor, Validated(payload): Validated<CreateUser>) -> impl IntoResponse {
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "email": payload.email,
            "phone": payload.phone,
        })),
    )
}
