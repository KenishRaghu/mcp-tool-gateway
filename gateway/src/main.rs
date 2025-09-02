use std::sync::Arc;

use mcp_gateway::{config::Config, creds::CredentialIssuer, mcp, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = telemetry::init();
    let config = Config::load()?;
    let listen = config.listen.clone();
    tracing::info!(
        listen = %listen,
        mode = ?config.mode,
        workspace = %config.workspace.display(),
        agents = config.agents.len(),
        "starting mcp-gateway"
    );

    let state = Arc::new(mcp::AppState::new(config, CredentialIssuer::from_env())?);
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    axum::serve(listener, mcp::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Flush buffered spans before exiting.
    if let Some(p) = provider {
        let _ = p.shutdown();
    }
    Ok(())
}

/// Resolves on Ctrl-C or SIGTERM (what ECS / docker / the eval runner send).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}
