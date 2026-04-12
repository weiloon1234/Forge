use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde::de::DeserializeOwned;
use tower::ServiceExt;

use crate::foundation::{App, AppBuilder, AppContext, Result};

/// A test application that bootstraps the full framework without starting a server.
///
/// ```ignore
/// let app = TestApp::builder()
///     .register_provider(MyProvider)
///     .register_routes(my_routes)
///     .build().await;
///
/// let response = app.client().get("/health").send().await;
/// assert_eq!(response.status(), 200);
/// ```
pub struct TestApp {
    app: AppContext,
    router: Router,
}

impl TestApp {
    /// Create a builder for configuring the test application.
    pub fn builder() -> TestAppBuilder {
        TestAppBuilder {
            inner: App::builder(),
            config_dir: None,
        }
    }

    /// Access the underlying AppContext for direct service resolution.
    pub fn app(&self) -> &AppContext {
        &self.app
    }

    /// Create a test HTTP client that sends requests to the app's router directly.
    pub fn client(&self) -> TestClient {
        TestClient {
            router: self.router.clone(),
        }
    }
}

/// Builder for TestApp.
pub struct TestAppBuilder {
    inner: AppBuilder,
    config_dir: Option<PathBuf>,
}

impl TestAppBuilder {
    pub fn load_config_dir(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        self.config_dir = Some(path.clone());
        self.inner = self.inner.load_config_dir(path);
        self
    }

    pub fn register_provider<P>(mut self, provider: P) -> Self
    where
        P: crate::foundation::ServiceProvider,
    {
        self.inner = self.inner.register_provider(provider);
        self
    }

    pub fn register_routes<F>(mut self, registrar: F) -> Self
    where
        F: Fn(&mut crate::http::HttpRegistrar) -> Result<()> + Send + Sync + 'static,
    {
        self.inner = self.inner.register_routes(registrar);
        self
    }

    pub fn register_middleware(mut self, config: crate::http::middleware::MiddlewareConfig) -> Self {
        self.inner = self.inner.register_middleware(config);
        self
    }

    /// Build the test application. Bootstraps all services without starting a server.
    pub async fn build(self) -> TestApp {
        let kernel = self
            .inner
            .build_http_kernel()
            .await
            .expect("failed to build test app");
        let router = kernel.build_router().expect("failed to build test router");
        TestApp {
            app: kernel.app().clone(),
            router,
        }
    }
}

/// HTTP test client that sends requests directly to the router without TCP.
#[derive(Clone)]
pub struct TestClient {
    router: Router,
}

impl TestClient {
    pub fn get(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.router.clone(), Method::GET, path)
    }

    pub fn post(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.router.clone(), Method::POST, path)
    }

    pub fn put(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.router.clone(), Method::PUT, path)
    }

    pub fn patch(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.router.clone(), Method::PATCH, path)
    }

    pub fn delete(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.router.clone(), Method::DELETE, path)
    }
}

/// Builder for constructing a test HTTP request.
pub struct TestRequestBuilder {
    router: Router,
    method: Method,
    path: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

impl TestRequestBuilder {
    fn new(router: Router, method: Method, path: &str) -> Self {
        Self {
            router,
            method,
            path: path.to_string(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// Add a header to the request.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Set the Authorization header with a bearer token.
    pub fn bearer_auth(self, token: &str) -> Self {
        self.header("authorization", &format!("Bearer {token}"))
    }

    /// Set a JSON request body.
    pub fn json(mut self, value: &impl serde::Serialize) -> Self {
        self.body = Some(serde_json::to_string(value).expect("failed to serialize JSON body"));
        self.headers
            .push(("content-type".to_string(), "application/json".to_string()));
        self
    }

    /// Send the request and return the response.
    pub async fn send(self) -> TestResponse {
        let body = match self.body {
            Some(b) => Body::from(b),
            None => Body::empty(),
        };

        let mut builder = Request::builder()
            .method(self.method)
            .uri(&self.path);

        for (name, value) in &self.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let request = builder.body(body).expect("failed to build request");
        let response = self
            .router
            .oneshot(request)
            .await
            .expect("failed to send request");

        let status = response.status();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("failed to read response body");

        TestResponse {
            status,
            headers,
            body: body_bytes.to_vec(),
        }
    }
}

/// A test HTTP response with convenience methods for assertions.
pub struct TestResponse {
    status: StatusCode,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl TestResponse {
    /// The HTTP status code.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Get a response header value by name.
    pub fn header(&self, name: &str) -> Option<&str> {
        let name_lower = name.to_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == name_lower)
            .map(|(_, v)| v.as_str())
    }

    /// Parse the response body as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).expect("failed to parse response as JSON")
    }

    /// The response body as a UTF-8 string.
    pub fn text(&self) -> String {
        String::from_utf8(self.body.clone()).expect("response body is not UTF-8")
    }

    /// The raw response body bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }
}
