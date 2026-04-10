use std::collections::{BTreeSet, HashMap};
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::{header, request::Parts, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::AuthConfig;
use crate::foundation::{AppContext, Error, Result};
use crate::support::{GuardId, PermissionId, PolicyId, RoleId};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AccessScope {
    #[default]
    Public,
    Guarded(GuardedAccess),
}

impl AccessScope {
    pub fn requires_auth(&self) -> bool {
        matches!(self, Self::Guarded(_))
    }

    pub fn guard(&self) -> Option<&GuardId> {
        match self {
            Self::Public => None,
            Self::Guarded(access) => access.guard.as_ref(),
        }
    }

    pub fn permissions(&self) -> BTreeSet<PermissionId> {
        match self {
            Self::Public => BTreeSet::new(),
            Self::Guarded(access) => access.permissions.clone(),
        }
    }

    pub fn with_guard<I>(mut self, guard: I) -> Self
    where
        I: Into<GuardId>,
    {
        self.guarded_mut().guard = Some(guard.into());
        self
    }

    pub fn with_permission<I>(mut self, permission: I) -> Self
    where
        I: Into<PermissionId>,
    {
        self.guarded_mut().permissions.insert(permission.into());
        self
    }

    pub fn with_permissions<I, P>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PermissionId>,
    {
        let access = self.guarded_mut();
        access.permissions = permissions.into_iter().map(Into::into).collect();
        self
    }

    fn guarded_mut(&mut self) -> &mut GuardedAccess {
        if !matches!(self, Self::Guarded(_)) {
            *self = Self::Guarded(GuardedAccess::default());
        }

        match self {
            Self::Public => unreachable!("access scope should be guarded"),
            Self::Guarded(access) => access,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardedAccess {
    pub guard: Option<GuardId>,
    pub permissions: BTreeSet<PermissionId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Actor {
    pub id: String,
    pub guard: GuardId,
    #[serde(default)]
    pub roles: BTreeSet<RoleId>,
    #[serde(default)]
    pub permissions: BTreeSet<PermissionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims: Option<Value>,
}

impl Actor {
    pub fn new<I, G>(id: I, guard: G) -> Self
    where
        I: Into<String>,
        G: Into<GuardId>,
    {
        Self {
            id: id.into(),
            guard: guard.into(),
            roles: BTreeSet::new(),
            permissions: BTreeSet::new(),
            claims: None,
        }
    }

    pub fn with_guard<I>(mut self, guard: I) -> Self
    where
        I: Into<GuardId>,
    {
        self.guard = guard.into();
        self
    }

    pub fn with_roles<I, R>(mut self, roles: I) -> Self
    where
        I: IntoIterator<Item = R>,
        R: Into<RoleId>,
    {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_permissions<I, P>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PermissionId>,
    {
        self.permissions = permissions.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_claims(mut self, claims: Value) -> Self {
        self.claims = Some(claims);
        self
    }

    pub fn has_role<I>(&self, role: I) -> bool
    where
        I: Into<RoleId>,
    {
        self.roles.contains(&role.into())
    }

    pub fn has_permission<I>(&self, permission: I) -> bool
    where
        I: Into<PermissionId>,
    {
        self.permissions.contains(&permission.into())
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum AuthError {
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Internal(String),
}

impl AuthError {
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Unauthorized(message.into())
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (
            self.status_code(),
            Json(serde_json::json!({
                "message": self.to_string(),
            })),
        )
            .into_response()
    }
}

#[async_trait]
pub trait BearerAuthenticator: Send + Sync + 'static {
    async fn authenticate(&self, token: &str) -> Result<Option<Actor>>;
}

#[async_trait]
pub trait Policy: Send + Sync + 'static {
    async fn evaluate(&self, actor: &Actor, app: &AppContext) -> Result<bool>;
}

pub(crate) type GuardRegistryHandle = Arc<Mutex<GuardRegistryBuilder>>;
pub(crate) type PolicyRegistryHandle = Arc<Mutex<PolicyRegistryBuilder>>;

#[derive(Default)]
pub(crate) struct GuardRegistryBuilder {
    guards: HashMap<GuardId, Arc<dyn BearerAuthenticator>>,
}

impl GuardRegistryBuilder {
    pub(crate) fn shared() -> GuardRegistryHandle {
        Arc::new(Mutex::new(Self::default()))
    }

    pub(crate) fn register_arc<I>(
        &mut self,
        id: I,
        guard: Arc<dyn BearerAuthenticator>,
    ) -> Result<()>
    where
        I: Into<GuardId>,
    {
        let id = id.into();
        if self.guards.contains_key(&id) {
            return Err(Error::message(format!(
                "auth guard `{id}` already registered"
            )));
        }
        self.guards.insert(id, guard);
        Ok(())
    }

    pub(crate) fn freeze_shared(
        handle: GuardRegistryHandle,
    ) -> HashMap<GuardId, Arc<dyn BearerAuthenticator>> {
        let mut builder = handle.lock().expect("guard registry lock poisoned");
        std::mem::take(&mut builder.guards)
    }
}

#[derive(Default)]
pub(crate) struct PolicyRegistryBuilder {
    policies: HashMap<PolicyId, Arc<dyn Policy>>,
}

impl PolicyRegistryBuilder {
    pub(crate) fn shared() -> PolicyRegistryHandle {
        Arc::new(Mutex::new(Self::default()))
    }

    pub(crate) fn register_arc<I>(&mut self, id: I, policy: Arc<dyn Policy>) -> Result<()>
    where
        I: Into<PolicyId>,
    {
        let id = id.into();
        if self.policies.contains_key(&id) {
            return Err(Error::message(format!(
                "auth policy `{id}` already registered"
            )));
        }
        self.policies.insert(id, policy);
        Ok(())
    }

    pub(crate) fn freeze_shared(
        handle: PolicyRegistryHandle,
    ) -> HashMap<PolicyId, Arc<dyn Policy>> {
        let mut builder = handle.lock().expect("policy registry lock poisoned");
        std::mem::take(&mut builder.policies)
    }
}

#[derive(Clone)]
pub struct AuthManager {
    config: AuthConfig,
    guards: Arc<HashMap<GuardId, Arc<dyn BearerAuthenticator>>>,
}

impl AuthManager {
    pub(crate) fn new(
        config: AuthConfig,
        guards: HashMap<GuardId, Arc<dyn BearerAuthenticator>>,
    ) -> Self {
        Self {
            config,
            guards: Arc::new(guards),
        }
    }

    pub fn default_guard(&self) -> &GuardId {
        &self.config.default_guard
    }

    pub async fn authenticate_headers(
        &self,
        headers: &HeaderMap,
        guard: Option<&GuardId>,
    ) -> std::result::Result<Actor, AuthError> {
        let token = self.extract_token(headers)?;
        self.authenticate_token(&token, guard).await
    }

    pub async fn authenticate_token(
        &self,
        token: &str,
        guard: Option<&GuardId>,
    ) -> std::result::Result<Actor, AuthError> {
        let guard_id = guard
            .cloned()
            .unwrap_or_else(|| self.default_guard().clone());
        let Some(authenticator) = self.guards.get(&guard_id).cloned() else {
            return Err(AuthError::internal(format!(
                "auth guard `{guard_id}` is not registered"
            )));
        };

        let Some(actor) = authenticator
            .authenticate(token)
            .await
            .map_err(|error| AuthError::internal(error.to_string()))?
        else {
            return Err(AuthError::unauthorized("invalid bearer token"));
        };

        Ok(actor.with_guard(guard_id))
    }

    pub fn extract_token(&self, headers: &HeaderMap) -> std::result::Result<String, AuthError> {
        let Some(value) = headers.get(header::AUTHORIZATION) else {
            return Err(AuthError::unauthorized("missing authorization header"));
        };
        let value = value
            .to_str()
            .map_err(|_| AuthError::unauthorized("invalid authorization header"))?;
        let prefix = self.config.bearer_prefix.trim();
        let expected = format!("{prefix} ");
        if !value
            .get(..expected.len())
            .map(|actual| actual.eq_ignore_ascii_case(&expected))
            .unwrap_or(false)
        {
            return Err(AuthError::unauthorized(format!(
                "authorization header must start with `{prefix}`"
            )));
        }
        let token = value[expected.len()..].trim();
        if token.is_empty() {
            return Err(AuthError::unauthorized("bearer token is missing"));
        }
        Ok(token.to_string())
    }
}

#[derive(Clone)]
pub struct Authorizer {
    app: AppContext,
    policies: Arc<HashMap<PolicyId, Arc<dyn Policy>>>,
}

impl Authorizer {
    pub(crate) fn new(app: AppContext, policies: HashMap<PolicyId, Arc<dyn Policy>>) -> Self {
        Self {
            app,
            policies: Arc::new(policies),
        }
    }

    pub fn allows_permission(&self, actor: &Actor, permission: &PermissionId) -> bool {
        actor.permissions.contains(permission)
    }

    pub fn allows_permissions(&self, actor: &Actor, permissions: &BTreeSet<PermissionId>) -> bool {
        permissions
            .iter()
            .all(|permission| self.allows_permission(actor, permission))
    }

    pub async fn authorize_permissions(
        &self,
        actor: &Actor,
        permissions: &BTreeSet<PermissionId>,
    ) -> std::result::Result<(), AuthError> {
        if self.allows_permissions(actor, permissions) {
            Ok(())
        } else {
            Err(AuthError::forbidden("missing required permission"))
        }
    }

    pub async fn allows_policy<I>(&self, actor: &Actor, policy: I) -> Result<bool>
    where
        I: Into<PolicyId>,
    {
        let policy = policy.into();
        let Some(policy_handler) = self.policies.get(&policy).cloned() else {
            return Err(Error::message(format!(
                "auth policy `{policy}` is not registered"
            )));
        };

        policy_handler.evaluate(actor, &self.app).await
    }
}

#[derive(Clone, Default)]
pub struct StaticBearerAuthenticator {
    actors: Arc<HashMap<String, Actor>>,
}

impl StaticBearerAuthenticator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn token(mut self, token: impl Into<String>, actor: Actor) -> Self {
        Arc::make_mut(&mut self.actors).insert(token.into(), actor);
        self
    }
}

#[async_trait]
impl BearerAuthenticator for StaticBearerAuthenticator {
    async fn authenticate(&self, token: &str) -> Result<Option<Actor>> {
        Ok(self.actors.get(token).cloned())
    }
}

#[derive(Debug, Clone)]
pub struct CurrentActor(pub Actor);

impl Deref for CurrentActor {
    type Target = Actor;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequestParts<S> for CurrentActor
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Actor>()
            .cloned()
            .map(Self)
            .ok_or_else(|| AuthError::unauthorized("authenticated actor not found").into_response())
    }
}

#[derive(Debug, Clone, Default)]
pub struct OptionalActor(pub Option<Actor>);

impl OptionalActor {
    pub fn as_ref(&self) -> Option<&Actor> {
        self.0.as_ref()
    }

    pub fn into_inner(self) -> Option<Actor> {
        self.0
    }
}

impl<S> FromRequestParts<S> for OptionalActor
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        Ok(Self(parts.extensions.get::<Actor>().cloned()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::http::{header, HeaderMap};

    use super::{
        Actor, AuthConfig, AuthManager, Authorizer, GuardRegistryBuilder, PermissionId,
        PolicyRegistryBuilder, StaticBearerAuthenticator,
    };
    use crate::config::ConfigRepository;
    use crate::foundation::{AppContext, Container};
    use crate::support::{GuardId, PolicyId, RoleId};
    use crate::validation::RuleRegistry;

    struct AllowEverythingPolicy;

    #[async_trait]
    impl super::Policy for AllowEverythingPolicy {
        async fn evaluate(&self, _actor: &super::Actor, _app: &AppContext) -> crate::Result<bool> {
            Ok(true)
        }
    }

    fn app() -> AppContext {
        AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        )
    }

    #[test]
    fn rejects_duplicate_guard_registration() {
        let guards = GuardRegistryBuilder::shared();
        guards
            .lock()
            .unwrap()
            .register_arc(
                GuardId::new("api"),
                Arc::new(StaticBearerAuthenticator::new()),
            )
            .unwrap();

        let error = guards
            .lock()
            .unwrap()
            .register_arc(
                GuardId::new("api"),
                Arc::new(StaticBearerAuthenticator::new()),
            )
            .unwrap_err();
        assert!(error.to_string().contains("already registered"));
    }

    #[test]
    fn rejects_duplicate_policy_registration() {
        let policies = PolicyRegistryBuilder::shared();
        policies
            .lock()
            .unwrap()
            .register_arc(PolicyId::new("admin"), Arc::new(AllowEverythingPolicy))
            .unwrap();

        let error = policies
            .lock()
            .unwrap()
            .register_arc(PolicyId::new("admin"), Arc::new(AllowEverythingPolicy))
            .unwrap_err();
        assert!(error.to_string().contains("already registered"));
    }

    #[tokio::test]
    async fn uses_default_guard_and_parses_bearer_header() {
        let actor = Actor::new("user-1", GuardId::new("placeholder"));
        let manager = AuthManager::new(
            AuthConfig::default(),
            HashMap::from([(
                GuardId::new("api"),
                Arc::new(StaticBearerAuthenticator::new().token("token-1", actor.clone()))
                    as Arc<dyn super::BearerAuthenticator>,
            )]),
        );

        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer token-1".parse().unwrap());
        let resolved = manager.authenticate_headers(&headers, None).await.unwrap();

        assert_eq!(resolved.id, actor.id);
        assert_eq!(resolved.guard, GuardId::new("api"));
    }

    #[tokio::test]
    async fn permission_checks_follow_actor_permissions() {
        let app = app();
        let authorizer = Authorizer::new(app, HashMap::new());
        let actor = Actor::new("user-1", GuardId::new("api"))
            .with_roles([RoleId::new("viewer")])
            .with_permissions([PermissionId::new("reports:view")]);

        let allowed = BTreeSet::from([PermissionId::new("reports:view")]);
        let denied = BTreeSet::from([PermissionId::new("admin")]);

        assert!(authorizer
            .authorize_permissions(&actor, &allowed)
            .await
            .is_ok());
        assert!(actor.has_role(RoleId::new("viewer")));
        assert!(authorizer
            .authorize_permissions(&actor, &denied)
            .await
            .is_err());
    }
}
