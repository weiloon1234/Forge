use std::collections::HashMap;
use std::fmt::Write as _;

use crate::foundation::{Error, Result};
use crate::support::RouteId;

/// Registry of named routes mapping names to their path patterns.
///
/// Used for URL generation: `app.route_url(Route::UsersShow, &[("id", "123")])`.
#[derive(Clone, Debug, Default)]
pub struct RouteRegistry {
    routes: HashMap<RouteId, String>,
}

impl RouteRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a named route with its path pattern.
    pub fn register(&mut self, name: impl Into<RouteId>, pattern: impl Into<String>) {
        self.routes.insert(name.into(), pattern.into());
    }

    /// Generate a URL from a named route, replacing `:param`, `{param}`, and `{*param}` segments.
    ///
    /// ```ignore
    /// let url = registry.url(Route::UsersShow, &[("id", "123")])?;
    /// // Returns: "/api/v1/users/123"
    /// ```
    pub fn url<I>(&self, name: I, params: &[(&str, &str)]) -> Result<String>
    where
        I: Into<RouteId>,
    {
        let name = name.into();
        let pattern = self
            .routes
            .get(&name)
            .ok_or_else(|| Error::message(format!("route '{}' not found", name.as_str())))?;

        let params_by_key = params.iter().copied().collect::<HashMap<_, _>>();
        let mut url = pattern.clone();
        for param in super::route_path_params(pattern) {
            let value = params_by_key.get(param.as_str()).ok_or_else(|| {
                Error::message(format!(
                    "route '{}' is missing required parameter `{param}`",
                    name.as_str()
                ))
            })?;
            let encoded = percent_encode(value);
            url = replace_route_param(&url, &param, &encoded);
        }
        Ok(url)
    }

    /// Check if a named route exists.
    pub fn has<I>(&self, name: I) -> bool
    where
        I: Into<RouteId>,
    {
        self.routes.contains_key(&name.into())
    }

    /// Iterate over all registered routes.
    pub fn iter(&self) -> impl Iterator<Item = (&RouteId, &String)> {
        self.routes.iter()
    }

    /// Generate a signed URL with HMAC-SHA256 signature and expiry timestamp.
    pub fn signed_url(
        &self,
        name: impl Into<RouteId>,
        params: &[(&str, &str)],
        signing_key: &[u8],
        expires_at: crate::support::DateTime,
    ) -> Result<String> {
        let mut url = self.url(name, params)?;
        let expiry = expires_at.as_chrono().timestamp();
        let sep = if url.contains('?') { "&" } else { "?" };
        url = format!("{url}{sep}expires={expiry}");
        let signature = crate::support::hmac::hmac_sha256_hex(signing_key, &url);
        Ok(format!("{url}&signature={signature}"))
    }

    /// Verify a signed URL's signature and expiry.
    pub fn verify_signature(url: &str, signing_key: &[u8]) -> Result<()> {
        // Find and extract the signature parameter
        let (url_without_sig, signature) = extract_signature_param(url)?;

        // Recompute HMAC
        let expected = crate::support::hmac::hmac_sha256_hex(signing_key, &url_without_sig);

        // Constant-time comparison
        if !crate::support::hmac::constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
            return Err(Error::http(403, "invalid signature"));
        }

        // Check expiry
        let expires = extract_expires_param(&url_without_sig)?;
        let now = chrono::Utc::now().timestamp();
        if now > expires {
            return Err(Error::http(403, "signed URL has expired"));
        }

        Ok(())
    }
}

fn replace_route_param(url: &str, param: &str, value: &str) -> String {
    url.replace(&format!("{{{param}}}"), value)
        .replace(&format!("{{*{param}}}"), value)
        .replace(&format!(":{param}"), value)
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn extract_signature_param(url: &str) -> Result<(String, String)> {
    if let Some(pos) = url.rfind("&signature=") {
        let signature = url[pos + 11..].to_string();
        let url_without = url[..pos].to_string();
        Ok((url_without, signature))
    } else if let Some(pos) = url.rfind("?signature=") {
        let signature = url[pos + 11..].to_string();
        let url_without = url[..pos].to_string();
        Ok((url_without, signature))
    } else {
        Err(Error::http(403, "missing signature"))
    }
}

fn extract_expires_param(url: &str) -> Result<i64> {
    let query = url.split('?').nth(1).unwrap_or("");
    for param in query.split('&') {
        if let Some(value) = param.strip_prefix("expires=") {
            return value
                .parse::<i64>()
                .map_err(|_| Error::http(403, "invalid expires parameter"));
        }
    }
    Err(Error::http(403, "missing expires parameter"))
}
