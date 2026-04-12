use std::sync::Arc;
use std::time::Duration;

use crate::database::{DatabaseManager, DbValue};
use crate::foundation::{Error, Result};
use crate::support::{DateTime, Token};

/// Shared token storage for password resets and email verification.
///
/// Both use the `password_reset_tokens` table, differentiated by the `guard`
/// column value.
pub(crate) struct TokenStore {
    database: Arc<DatabaseManager>,
    expiry: Duration,
    kind: &'static str,
}

impl TokenStore {
    pub fn new(database: Arc<DatabaseManager>, expiry: Duration, kind: &'static str) -> Self {
        Self {
            database,
            expiry,
            kind,
        }
    }

    pub async fn create_token(&self, email: &str, guard: String) -> Result<String> {
        let plaintext = Token::base64(32)?;
        let hash = crate::support::sha256_hex_str(&plaintext);

        self.database
            .raw_execute(
                "INSERT INTO password_reset_tokens (email, guard, token_hash, created_at) \
                 VALUES ($1, $2, $3, NOW()) \
                 ON CONFLICT (email, guard) DO UPDATE SET token_hash = $3, created_at = NOW()",
                &[
                    DbValue::Text(email.to_string()),
                    DbValue::Text(guard),
                    DbValue::Text(hash),
                ],
            )
            .await?;

        Ok(plaintext)
    }

    pub async fn validate_token(
        &self,
        email: &str,
        token: &str,
        guard: String,
    ) -> Result<()> {
        let hash = crate::support::sha256_hex_str(token);
        let expiry_seconds = self.expiry.as_secs() as i64;

        let rows = self
            .database
            .raw_query(
                "SELECT token_hash, created_at FROM password_reset_tokens \
                 WHERE email = $1 AND guard = $2",
                &[
                    DbValue::Text(email.to_string()),
                    DbValue::Text(guard.clone()),
                ],
            )
            .await?;

        let invalid_msg = format!("invalid or expired {} token", self.kind);

        let row = rows
            .first()
            .ok_or_else(|| Error::message(&invalid_msg))?;

        let stored_hash = match row.get("token_hash") {
            Some(DbValue::Text(h)) => h.clone(),
            _ => return Err(Error::message(&invalid_msg)),
        };

        if stored_hash != hash {
            return Err(Error::message(&invalid_msg));
        }

        let created_at = match row.get("created_at") {
            Some(DbValue::TimestampTz(ts)) => *ts,
            _ => return Err(Error::message(&invalid_msg)),
        };

        let now = DateTime::now();
        if (now.as_chrono() - created_at.as_chrono()).num_seconds() > expiry_seconds {
            self.delete_token(email, &guard).await?;
            return Err(Error::message(format!("{} token has expired", self.kind)));
        }

        self.delete_token(email, &guard).await?;
        Ok(())
    }

    pub async fn delete_token(&self, email: &str, guard: &str) -> Result<()> {
        self.database
            .raw_execute(
                "DELETE FROM password_reset_tokens WHERE email = $1 AND guard = $2",
                &[
                    DbValue::Text(email.to_string()),
                    DbValue::Text(guard.to_string()),
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn prune_expired(&self, guard_filter: Option<&str>) -> Result<u64> {
        let expiry_seconds = self.expiry.as_secs() as i64;
        let sql = match guard_filter {
            Some(filter) => format!(
                "DELETE FROM password_reset_tokens \
                 WHERE guard LIKE '{filter}%' \
                 AND created_at < NOW() - INTERVAL '{expiry_seconds} seconds'"
            ),
            None => format!(
                "DELETE FROM password_reset_tokens \
                 WHERE created_at < NOW() - INTERVAL '{expiry_seconds} seconds'"
            ),
        };
        self.database.raw_execute(&sql, &[]).await
    }
}
