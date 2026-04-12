pub use axum_extra::extract::cookie::{Cookie, SameSite};
pub use axum_extra::extract::CookieJar;

/// Helpers for building secure session cookies.
pub struct SessionCookie;

impl SessionCookie {
    /// Build a session cookie with secure defaults:
    /// HttpOnly, SameSite=Lax, Path=/, and optionally Secure.
    pub fn build<'a>(name: &'a str, value: &'a str, secure: bool) -> Cookie<'a> {
        let mut builder = Cookie::build((name, value))
            .http_only(true)
            .same_site(SameSite::Lax)
            .path("/");

        if secure {
            builder = builder.secure(true);
        }

        builder.build()
    }

    /// Build an expired removal cookie (clears the cookie on the client).
    pub fn clear(name: &str) -> Cookie<'_> {
        let mut cookie = Cookie::build(name)
            .http_only(true)
            .same_site(SameSite::Lax)
            .path("/")
            .build();
        cookie.make_removal();
        cookie
    }
}
