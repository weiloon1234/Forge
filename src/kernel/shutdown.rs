use tokio::signal;

/// Wait for SIGTERM or SIGINT (Ctrl+C) to initiate graceful shutdown.
pub(crate) async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => { tracing::info!("forge: received SIGINT, shutting down gracefully"); }
        _ = terminate => { tracing::info!("forge: received SIGTERM, shutting down gracefully"); }
    }
}
