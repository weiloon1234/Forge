use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use clap::{Arg, Command};

use crate::cli::{CommandInvocation, CommandRegistrar};
use crate::config::TokenConfig;
use crate::database::{DatabaseManager, DbValue, FromDbValue};
use crate::foundation::{AppContext, Error, Result};
use crate::support::{sha256_hex_str, CommandId, GuardId, PermissionId, Token};

use super::{Actor, Authenticatable, BearerAuthenticator};

const TOKEN_PRUNE_COMMAND: CommandId = CommandId::new("token:prune");

/// A pair of access + refresh tokens returned to the client after login.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS, forge_macros::TS, forge_macros::ApiSchema)]
#[ts(export)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    #[ts(type = "number")]
    pub expires_in: u64,
    pub token_type: String,
}

/// Manages personal access tokens: issuance, validation, refresh, and revocation.
///
/// Stored as a singleton in the container, accessible via `app.tokens()`.
pub struct TokenManager {
    db: Arc<DatabaseManager>,
    config: TokenConfig,
}

impl TokenManager {
    pub(crate) fn new(db: Arc<DatabaseManager>, config: TokenConfig) -> Self {
        Self { db, config }
    }

    /// Issue a new access + refresh token pair for the given actor.
    pub async fn issue<M: Authenticatable>(&self, actor_id: &str) -> Result<TokenPair> {
        self.issue_named::<M>(actor_id, "").await
    }

    /// Issue a new token pair with a human-readable name (e.g., "My iPhone", "CLI").
    pub async fn issue_named<M: Authenticatable>(
        &self,
        actor_id: &str,
        name: &str,
    ) -> Result<TokenPair> {
        self.insert_token_pair(M::guard().as_ref(), actor_id, name, &[])
            .await
    }

    /// Issue a new token pair with scoped abilities.
    ///
    /// Abilities are stored as a JSON array on the token row and automatically
    /// become [`Actor`] permissions when the token is validated, integrating
    /// with the existing permission system.
    ///
    /// ```ignore
    /// let pair = app.tokens()?.issue_with_abilities::<User>(
    ///     &user.id.to_string(),
    ///     "mobile-app",
    ///     vec!["orders:read".into(), "profile:write".into()],
    /// ).await?;
    /// ```
    pub async fn issue_with_abilities<M: Authenticatable>(
        &self,
        actor_id: &str,
        name: &str,
        abilities: Vec<String>,
    ) -> Result<TokenPair> {
        self.insert_token_pair(M::guard().as_ref(), actor_id, name, &abilities)
            .await
    }

    /// Validate an access token and return the Actor if valid.
    ///
    /// Read-only — does not write on every request. Use [`touch`] to update
    /// `last_used_at` if needed for auditing.
    pub async fn validate(&self, access_token: &str) -> Result<Option<Actor>> {
        let hash = sha256_hex_str(access_token);

        let rows = self
            .db
            .raw_query(
                r#"
                SELECT guard, actor_id, abilities
                FROM personal_access_tokens
                WHERE access_token_hash = $1
                  AND revoked_at IS NULL
                  AND expires_at > NOW()
                "#,
                &[DbValue::Text(hash)],
            )
            .await?;

        let Some(row) = rows.first() else {
            return Ok(None);
        };

        let guard = String::from_db_value(
            row.get("guard")
                .ok_or_else(|| Error::message("missing guard column"))?,
        )?;
        let actor_id = String::from_db_value(
            row.get("actor_id")
                .ok_or_else(|| Error::message("missing actor_id column"))?,
        )?;

        let mut actor = Actor::new(actor_id, GuardId::owned(guard));

        // Parse token-scoped abilities into Actor permissions.
        if let Some(abilities_value) = row.get("abilities") {
            if let Ok(abilities_json) = serde_json::Value::from_db_value(abilities_value) {
                if let Ok(abilities) =
                    serde_json::from_value::<Vec<String>>(abilities_json)
                {
                    actor = actor.with_permissions(
                        abilities.iter().map(|a| PermissionId::owned(a.clone())),
                    );
                }
            }
        }

        Ok(Some(actor))
    }

    /// Update `last_used_at` for a token. Call this explicitly when you need
    /// usage tracking — it is not called automatically on every request.
    pub async fn touch(&self, access_token: &str) -> Result<()> {
        let hash = sha256_hex_str(access_token);
        self.db
            .raw_execute(
                "UPDATE personal_access_tokens SET last_used_at = NOW() WHERE access_token_hash = $1 AND revoked_at IS NULL",
                &[DbValue::Text(hash)],
            )
            .await?;
        Ok(())
    }

    /// Refresh a token pair using a valid refresh token.
    ///
    /// Atomically revokes the old token (if rotation enabled) and returns the
    /// actor info needed to issue a new pair. A stolen refresh token can only
    /// be used once — concurrent use of the same token will fail for the loser.
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair> {
        let hash = sha256_hex_str(refresh_token);

        // Atomic: revoke + return in one query to prevent concurrent reuse
        let rows = if self.config.rotate_refresh_tokens {
            self.db
                .raw_query(
                    r#"
                    UPDATE personal_access_tokens
                    SET revoked_at = NOW()
                    WHERE refresh_token_hash = $1
                      AND revoked_at IS NULL
                      AND refresh_expires_at > NOW()
                    RETURNING guard, actor_id
                    "#,
                    &[DbValue::Text(hash)],
                )
                .await?
        } else {
            self.db
                .raw_query(
                    r#"
                    SELECT guard, actor_id
                    FROM personal_access_tokens
                    WHERE refresh_token_hash = $1
                      AND revoked_at IS NULL
                      AND refresh_expires_at > NOW()
                    "#,
                    &[DbValue::Text(hash)],
                )
                .await?
        };

        let row = rows
            .first()
            .ok_or_else(|| Error::message("invalid or expired refresh token"))?;

        let guard = String::from_db_value(
            row.get("guard")
                .ok_or_else(|| Error::message("missing guard column"))?,
        )?;
        let actor_id = String::from_db_value(
            row.get("actor_id")
                .ok_or_else(|| Error::message("missing actor_id column"))?,
        )?;

        self.insert_token_pair(&guard, &actor_id, "", &[]).await
    }

    /// Revoke a specific access token.
    pub async fn revoke(&self, access_token: &str) -> Result<()> {
        let hash = sha256_hex_str(access_token);
        self.db
            .raw_execute(
                "UPDATE personal_access_tokens SET revoked_at = NOW() WHERE access_token_hash = $1 AND revoked_at IS NULL",
                &[DbValue::Text(hash)],
            )
            .await?;
        Ok(())
    }

    /// Revoke all tokens for an actor under a specific guard. Returns count revoked.
    pub async fn revoke_all<M: Authenticatable>(&self, actor_id: &str) -> Result<u64> {
        let guard = M::guard();
        self.db
            .raw_execute(
                "UPDATE personal_access_tokens SET revoked_at = NOW() WHERE guard = $1 AND actor_id = $2 AND revoked_at IS NULL",
                &[
                    DbValue::Text(guard.to_string()),
                    DbValue::Text(actor_id.to_string()),
                ],
            )
            .await
    }

    /// Delete tokens that are expired or revoked older than the given age.
    ///
    /// Returns the number of tokens deleted.
    pub async fn prune(&self, older_than_days: u64) -> Result<u64> {
        self.db
            .raw_execute(
                r#"
                DELETE FROM personal_access_tokens
                WHERE (revoked_at IS NOT NULL AND revoked_at < NOW() - $1 * INTERVAL '1 day')
                   OR (expires_at < NOW() - $1 * INTERVAL '1 day')
                "#,
                &[DbValue::Int64(older_than_days as i64)],
            )
            .await
    }

    /// Core token pair creation — shared by issue and refresh.
    async fn insert_token_pair(
        &self,
        guard: &str,
        actor_id: &str,
        name: &str,
        abilities: &[String],
    ) -> Result<TokenPair> {
        let access_plain = Token::base64(self.config.token_length)?;
        let refresh_plain = Token::base64(self.config.token_length)?;

        let access_hash = sha256_hex_str(&access_plain);
        let refresh_hash = sha256_hex_str(&refresh_plain);

        let expires_in_secs = self.config.access_token_ttl_minutes * 60;
        let refresh_expires_in_secs = self.config.refresh_token_ttl_days * 24 * 60 * 60;

        let abilities_json = serde_json::Value::Array(
            abilities
                .iter()
                .map(|a| serde_json::Value::String(a.clone()))
                .collect(),
        );

        self.db
            .raw_execute(
                r#"
                INSERT INTO personal_access_tokens
                    (guard, actor_id, name, access_token_hash, refresh_token_hash, abilities, expires_at, refresh_expires_at)
                VALUES
                    ($1, $2, $3, $4, $5, $6, NOW() + $7 * INTERVAL '1 second', NOW() + $8 * INTERVAL '1 second')
                "#,
                &[
                    DbValue::Text(guard.to_string()),
                    DbValue::Text(actor_id.to_string()),
                    DbValue::Text(name.to_string()),
                    DbValue::Text(access_hash),
                    DbValue::Text(refresh_hash),
                    DbValue::Json(abilities_json),
                    DbValue::Int64(expires_in_secs as i64),
                    DbValue::Int64(refresh_expires_in_secs as i64),
                ],
            )
            .await?;

        Ok(TokenPair {
            access_token: access_plain,
            refresh_token: refresh_plain,
            expires_in: expires_in_secs,
            token_type: "Bearer".to_string(),
        })
    }
}

/// A [`BearerAuthenticator`] that validates access tokens from the `personal_access_tokens` table.
///
/// Auto-created during bootstrap for guards with `driver = "token"` in config.
pub struct TokenAuthenticator {
    manager: Arc<TokenManager>,
}

impl TokenAuthenticator {
    pub fn new(manager: Arc<TokenManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl BearerAuthenticator for TokenAuthenticator {
    async fn authenticate(&self, token: &str) -> Result<Option<Actor>> {
        self.manager.validate(token).await
    }
}

pub(crate) fn builtin_cli_registrar() -> CommandRegistrar {
    Arc::new(|registry| {
        registry.command(
            TOKEN_PRUNE_COMMAND,
            Command::new(TOKEN_PRUNE_COMMAND.as_str().to_string())
                .about("Delete expired and revoked personal access tokens")
                .arg(
                    Arg::new("days")
                        .long("days")
                        .value_name("DAYS")
                        .default_value("30")
                        .help("Delete tokens expired/revoked more than this many days ago"),
                ),
            |invocation| async move { token_prune_command(invocation).await },
        )?;
        Ok(())
    })
}

async fn token_prune_command(invocation: CommandInvocation) -> Result<()> {
    let days_str = invocation
        .matches()
        .get_one::<String>("days")
        .map(|s| s.as_str())
        .unwrap_or("30");
    let days: u64 = days_str
        .parse()
        .map_err(|_| Error::message("--days must be a positive integer"))?;

    let tokens = invocation.app().tokens()?;
    let deleted = tokens.prune(days).await?;
    println!("pruned {deleted} token(s) older than {days} day(s)");
    Ok(())
}

// ---------------------------------------------------------------------------
// HasToken trait — Laravel-style HasApiTokens for Authenticatable models
// ---------------------------------------------------------------------------

/// Trait for models that can issue and manage personal access tokens.
///
/// Provides convenient instance methods for token CRUD, similar to
/// Laravel's `HasApiTokens` trait.
///
/// ```ignore
/// impl HasToken for User {}  // uses Authenticatable::guard() automatically
///
/// let pair = user.create_token(&app).await?;
/// let pair = user.create_token_named(&app, "My iPhone").await?;
/// let pair = user.create_token_with_abilities(&app, "ci", vec!["deploy:read".into()]).await?;
/// user.revoke_all_tokens(&app).await?;
/// ```
#[async_trait::async_trait]
pub trait HasToken: super::Authenticatable {
    /// Issue a new access + refresh token pair.
    async fn create_token(&self, app: &AppContext) -> Result<TokenPair> {
        let tokens = app.tokens()?;
        let id = self.token_actor_id();
        tokens.issue::<Self>(&id).await
    }

    /// Issue a named token pair (e.g., "My iPhone", "CLI").
    async fn create_token_named(&self, app: &AppContext, name: &str) -> Result<TokenPair> {
        let tokens = app.tokens()?;
        let id = self.token_actor_id();
        tokens.issue_named::<Self>(&id, name).await
    }

    /// Issue a token pair with scoped abilities.
    async fn create_token_with_abilities(
        &self,
        app: &AppContext,
        name: &str,
        abilities: Vec<String>,
    ) -> Result<TokenPair> {
        let tokens = app.tokens()?;
        let id = self.token_actor_id();
        tokens.issue_with_abilities::<Self>(&id, name, abilities).await
    }

    /// Revoke all tokens for this model instance.
    async fn revoke_all_tokens(&self, app: &AppContext) -> Result<u64> {
        let tokens = app.tokens()?;
        let id = self.token_actor_id();
        tokens.revoke_all::<Self>(&id).await
    }

    /// The actor ID used for token operations. Override if your model's
    /// primary key field is not named `id` or needs special formatting.
    fn token_actor_id(&self) -> String;
}
