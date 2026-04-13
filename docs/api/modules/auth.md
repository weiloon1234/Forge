# auth

Auth: guards, policies, tokens, sessions, password reset, email verification

[Back to index](../index.md)

## forge::auth

```rust
pub type Auth<M> = AuthenticatedModel<M>;
enum AccessScope { Public, Guarded }
  fn requires_auth(&self) -> bool
  fn guard(&self) -> Option<&GuardId>
  fn permissions(&self) -> BTreeSet<PermissionId>
  fn with_guard<I>(self, guard: I) -> Self
  fn with_permission<I>(self, permission: I) -> Self
  fn with_permissions<I, P>(self, permissions: I) -> Self
enum AuthError { Unauthorized, Forbidden, Internal }
  fn unauthorized(message: impl Into<String>) -> Self
  fn forbidden(message: impl Into<String>) -> Self
  fn internal(message: impl Into<String>) -> Self
  fn status_code(&self) -> StatusCode
struct Actor
  fn new<I, G>(id: I, guard: G) -> Self
  fn with_guard<I>(self, guard: I) -> Self
  fn with_roles<I, R>(self, roles: I) -> Self
  fn with_permissions<I, P>(self, permissions: I) -> Self
  fn with_claims(self, claims: Value) -> Self
  fn has_role<I>(&self, role: I) -> bool
  fn has_permission<I>(&self, permission: I) -> bool
  async fn resolve<M>(&self, app: &AppContext) -> Result<Option<M>>
struct AuthManager
  fn default_guard(&self) -> &GuardId
  async fn authenticate_headers( &self, headers: &HeaderMap, guard: Option<&GuardId>, ) -> Result<Actor, AuthError>
  async fn authenticate_token( &self, token: &str, guard: Option<&GuardId>, ) -> Result<Actor, AuthError>
  fn extract_token(&self, headers: &HeaderMap) -> Result<String, AuthError>
struct AuthenticatableRegistry
  async fn resolve_dynamic( &self, actor: &Actor, app: &AppContext, ) -> Result<Option<Box<dyn Any + Send + Sync>>>
  fn contains_guard(&self, guard: &GuardId) -> bool
struct AuthenticatedModel
struct Authorizer
  fn allows_permission( &self, actor: &Actor, permission: &PermissionId, ) -> bool
  fn allows_permissions( &self, actor: &Actor, permissions: &BTreeSet<PermissionId>, ) -> bool
  async fn authorize_permissions( &self, actor: &Actor, permissions: &BTreeSet<PermissionId>, ) -> Result<(), AuthError>
  async fn allows_policy<I>(&self, actor: &Actor, policy: I) -> Result<bool>
struct CurrentActor
struct GuardedAccess
struct OptionalActor
  fn as_ref(&self) -> Option<&Actor>
  fn into_inner(self) -> Option<Actor>
struct StaticBearerAuthenticator
  fn new() -> Self
  fn token(self, token: impl Into<String>, actor: Actor) -> Self
trait Authenticatable
  fn guard() -> GuardId
  fn resolve_from_actor<'life0, 'life1, 'async_trait, E>(
trait BearerAuthenticator
  fn authenticate<'life0, 'life1, 'async_trait>(
trait Policy
  fn evaluate<'life0, 'life1, 'life2, 'async_trait>(
```

## forge::auth::email_verification

```rust
struct EmailVerificationManager
  async fn create_token<M: Authenticatable>( &self, email: &str, ) -> Result<String>
  async fn validate_token<M: Authenticatable>( &self, email: &str, token: &str, ) -> Result<()>
  async fn prune_expired(&self) -> Result<u64>
```

## forge::auth::password_reset

```rust
struct PasswordResetManager
  async fn create_token<M: Authenticatable>( &self, email: &str, ) -> Result<String>
  async fn validate_token<M: Authenticatable>( &self, email: &str, token: &str, ) -> Result<()>
  async fn prune_expired(&self) -> Result<u64>
```

## forge::auth::session

```rust
struct SessionManager
  fn config(&self) -> &SessionConfig
  async fn create<M: Authenticatable>(&self, actor_id: &str) -> Result<String>
  async fn create_with_remember<M: Authenticatable>( &self, actor_id: &str, remember: bool, ) -> Result<String>
  async fn validate(&self, session_id: &str) -> Result<Option<Actor>>
  async fn destroy(&self, session_id: &str) -> Result<()>
  async fn destroy_all<M: Authenticatable>( &self, actor_id: &str, ) -> Result<()>
  fn login_response( &self, session_id: String, body: impl IntoResponse, ) -> Result<Response>
  fn logout_response(&self, body: impl IntoResponse) -> Result<Response>
```

## forge::auth::token

```rust
struct TokenAuthenticator
  fn new(manager: Arc<TokenManager>) -> Self
struct TokenManager
  async fn issue<M: Authenticatable>( &self, actor_id: &str, ) -> Result<TokenPair>
  async fn issue_named<M: Authenticatable>( &self, actor_id: &str, name: &str, ) -> Result<TokenPair>
  async fn issue_with_abilities<M: Authenticatable>( &self, actor_id: &str, name: &str, abilities: Vec<String>, ) -> Result<TokenPair>
  async fn validate(&self, access_token: &str) -> Result<Option<Actor>>
  async fn touch(&self, access_token: &str) -> Result<()>
  async fn refresh(&self, refresh_token: &str) -> Result<TokenPair>
  async fn revoke(&self, access_token: &str) -> Result<()>
  async fn revoke_all<M: Authenticatable>( &self, actor_id: &str, ) -> Result<u64>
  async fn prune(&self, older_than_days: u64) -> Result<u64>
struct TokenPair
trait HasToken: Authenticatable
  fn token_actor_id(&self) -> String
  fn create_token<'life0, 'life1, 'async_trait>(
  fn create_token_named<'life0, 'life1, 'life2, 'async_trait>(
  fn create_token_with_abilities<'life0, 'life1, 'life2, 'async_trait>(
  fn revoke_all_tokens<'life0, 'life1, 'async_trait>(
```

