use std::sync::Arc;

use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::config::SessionConfig;
use crate::foundation::{Error, Result};
use crate::http::cookie::SessionCookie;
use crate::redis::RedisManager;
use crate::support::{GuardId, Token};

use super::Actor;
use super::Authenticatable;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionData {
    actor_id: String,
    guard: String,
}

/// Manages Redis-backed sessions for web dashboard authentication.
///
/// Stored as a singleton in the container, accessible via `app.sessions()`.
pub struct SessionManager {
    redis: Arc<RedisManager>,
    config: SessionConfig,
}

impl SessionManager {
    pub(crate) fn new(redis: Arc<RedisManager>, config: SessionConfig) -> Self {
        Self { redis, config }
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// Create a new session for the given actor. Returns the session ID.
    pub async fn create<M: Authenticatable>(&self, actor_id: &str) -> Result<String> {
        self.create_with_remember::<M>(actor_id, false).await
    }

    /// Create a new session with optional "remember me" extended lifetime.
    ///
    /// When `remember` is `true`, the session uses the extended TTL
    /// (`remember_ttl_days`) instead of the standard `ttl_minutes`.
    pub async fn create_with_remember<M: Authenticatable>(
        &self,
        actor_id: &str,
        remember: bool,
    ) -> Result<String> {
        let session_id = Token::base64(32)?;
        let guard = M::guard();
        let data = SessionData {
            actor_id: actor_id.to_string(),
            guard: guard.to_string(),
        };
        let json = serde_json::to_string(&data).map_err(Error::other)?;
        let ttl_secs = if remember {
            self.config.remember_ttl_days * 24 * 60 * 60
        } else {
            self.config.ttl_minutes * 60
        };

        let mut conn = self.redis.connection().await?;
        let session_key = self.redis.key(format!("session:{session_id}"));
        conn.set_ex(&session_key, &json, ttl_secs).await?;

        let index_key = self.redis.key(format!("session_index:{guard}:{actor_id}"));
        conn.sadd(&index_key, &session_id).await?;

        Ok(session_id)
    }

    /// Validate a session ID and return the Actor if valid.
    /// Extends TTL if sliding expiry is enabled.
    pub async fn validate(&self, session_id: &str) -> Result<Option<Actor>> {
        let mut conn = self.redis.connection().await?;
        let session_key = self.redis.key(format!("session:{session_id}"));

        let json: String = match conn.get(&session_key).await {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };

        if json.is_empty() {
            return Ok(None);
        }

        let data: SessionData = serde_json::from_str(&json).map_err(Error::other)?;

        if self.config.sliding_expiry {
            let ttl_secs = self.config.ttl_minutes * 60;
            conn.expire(&session_key, ttl_secs).await?;
        }

        Ok(Some(Actor::new(data.actor_id, GuardId::owned(data.guard))))
    }

    /// Destroy a specific session.
    pub async fn destroy(&self, session_id: &str) -> Result<()> {
        let mut conn = self.redis.connection().await?;
        let session_key = self.redis.key(format!("session:{session_id}"));

        let json: String = match conn.get(&session_key).await {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };

        conn.del(&session_key).await?;

        if !json.is_empty() {
            if let Ok(data) = serde_json::from_str::<SessionData>(&json) {
                let index_key = self
                    .redis
                    .key(format!("session_index:{}:{}", data.guard, data.actor_id));
                conn.srem(&index_key, session_id).await?;
            }
        }

        Ok(())
    }

    /// Destroy all sessions for an actor under a specific guard.
    pub async fn destroy_all<M: Authenticatable>(&self, actor_id: &str) -> Result<()> {
        let guard = M::guard();
        let mut conn = self.redis.connection().await?;
        let index_key = self.redis.key(format!("session_index:{guard}:{actor_id}"));

        let session_ids: Vec<String> = conn.smembers(&index_key).await.unwrap_or_default();

        let session_keys: Vec<_> = session_ids
            .iter()
            .map(|sid| self.redis.key(format!("session:{sid}")))
            .collect();

        let all_keys: Vec<&_> = session_keys
            .iter()
            .chain(std::iter::once(&index_key))
            .collect();
        conn.del_many(&all_keys).await?;
        Ok(())
    }

    /// Extract session ID from request headers by parsing the Cookie header.
    pub(crate) fn extract_session_id(&self, headers: &HeaderMap) -> Option<String> {
        crate::http::cookie::extract_cookie_value(headers, &self.config.cookie_name)
    }

    /// Build a response that sets the session cookie alongside the given body.
    pub fn login_response(&self, session_id: String, body: impl IntoResponse) -> Result<Response> {
        let cookie = SessionCookie::build(
            &self.config.cookie_name,
            &session_id,
            self.config.cookie_secure,
        );
        self.with_cookie_header(cookie, body)
    }

    /// Build a response that clears the session cookie.
    pub fn logout_response(&self, body: impl IntoResponse) -> Result<Response> {
        let cookie = SessionCookie::clear(&self.config.cookie_name);
        self.with_cookie_header(cookie, body)
    }

    fn with_cookie_header(
        &self,
        cookie: crate::http::cookie::Cookie<'_>,
        body: impl IntoResponse,
    ) -> Result<Response> {
        let header_value = HeaderValue::from_str(&cookie.to_string())
            .map_err(|e| Error::message(format!("invalid session cookie value: {e}")))?;
        let mut response = body.into_response();
        response
            .headers_mut()
            .append(header::SET_COOKIE, header_value);
        Ok(response)
    }
}
