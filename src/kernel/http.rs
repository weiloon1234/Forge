use std::net::SocketAddr;

use tokio::net::TcpListener;

use crate::config::ServerConfig;
use crate::foundation::{AppContext, Error, Result};
use crate::http::middleware::MiddlewareConfig;
use crate::http::{HttpRegistrar, RouteRegistrar};
use crate::logging::ObservabilityOptions;

pub struct HttpKernel {
    app: AppContext,
    routes: Vec<RouteRegistrar>,
    middlewares: Vec<MiddlewareConfig>,
    observability: Option<ObservabilityOptions>,
}

impl HttpKernel {
    pub fn new(
        app: AppContext,
        routes: Vec<RouteRegistrar>,
        middlewares: Vec<MiddlewareConfig>,
        observability: Option<ObservabilityOptions>,
    ) -> Self {
        Self {
            app,
            routes,
            middlewares,
            observability,
        }
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }

    pub fn build_router(&self) -> Result<axum::Router> {
        let mut registrar = HttpRegistrar::new();
        for routes in &self.routes {
            routes(&mut registrar)?;
        }
        if let Some(options) = &self.observability {
            crate::logging::register_observability_routes(
                &mut registrar,
                &self.app.config().observability()?,
                options,
            )?;
        }
        Ok(registrar.into_router_with_middlewares(self.app.clone(), self.middlewares.clone()))
    }

    pub async fn bind(self) -> Result<BoundHttpServer> {
        let server = self.app.config().server()?;
        let listener = bind_listener(&server).await?;
        let local_addr = listener.local_addr().map_err(Error::other)?;
        let router = self.build_router()?;

        Ok(BoundHttpServer {
            listener,
            router,
            local_addr,
        })
    }

    pub async fn serve(self) -> Result<()> {
        self.bind().await?.serve().await
    }
}

pub struct BoundHttpServer {
    listener: TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
}

impl BoundHttpServer {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn serve(self) -> Result<()> {
        axum::serve(self.listener, self.router)
            .await
            .map_err(Error::other)
    }
}

async fn bind_listener(server: &ServerConfig) -> Result<TcpListener> {
    let addr = format!("{}:{}", server.host, server.port);
    TcpListener::bind(addr).await.map_err(Error::other)
}
