use std::sync::Arc;
use std::time::Duration;

use crate::database::DatabaseManager;
use crate::foundation::Result;

use super::token_store::TokenStore;
use super::Authenticatable;

/// Manages password reset token generation and validation.
///
/// Access via `app.password_resets()`.
pub struct PasswordResetManager {
    store: TokenStore,
}

impl PasswordResetManager {
    pub(crate) fn new(database: Arc<DatabaseManager>, expiry_minutes: u64) -> Self {
        Self {
            store: TokenStore::new(database, Duration::from_secs(expiry_minutes * 60), "reset"),
        }
    }

    /// Generate a password reset token for the given email.
    ///
    /// Returns the plaintext token (to be sent to the user).
    /// The token hash is stored in the database.
    pub async fn create_token<M: Authenticatable>(&self, email: &str) -> Result<String> {
        self.store.create_token(email, M::guard().to_string()).await
    }

    /// Validate a password reset token.
    ///
    /// Returns `Ok(())` if the token is valid and not expired.
    /// Deletes the token after successful validation (single use).
    pub async fn validate_token<M: Authenticatable>(&self, email: &str, token: &str) -> Result<()> {
        self.store
            .validate_token(email, token, M::guard().to_string())
            .await
    }

    /// Remove all expired tokens from the database.
    pub async fn prune_expired(&self) -> Result<u64> {
        self.store.prune_expired(None).await
    }
}
