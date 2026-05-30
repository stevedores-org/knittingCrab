use axum::{routing::get, Json, Router};
use knitting_crab_coordinator::{CoordinatorServer, CoordinatorState};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("Starting knittingCrab coordinator");

    let state = Arc::new(CoordinatorState::new());

    // Main coordinator address (custom TCP protocol)
    let coordinator_addr: SocketAddr = std::env::var("COORDINATOR_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:4000".to_string())
        .parse()?;

    // Health probe address (HTTP)
    let health_addr: SocketAddr = std::env::var("HEALTH_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse()?;

    // Setup health routes
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/healthz", get(liveness))
        .route("/readyz", get(readiness))
        .with_state(Arc::clone(&state));

    // Spawn coordinator server
    let coordinator_state = (*state).clone();
    tokio::spawn(async move {
        let server = CoordinatorServer::new(coordinator_state, coordinator_addr);
        if let Err(e) = server.serve().await {
            warn!("Coordinator server error: {}", e);
        }
    });

    // Start health HTTP server
    info!("Health probes listening on {}", health_addr);
    let listener = TcpListener::bind(health_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "knitting-crab-coordinator"
    }))
}

async fn liveness() -> &'static str {
    "OK"
}

async fn readiness() -> &'static str {
    // For now, always ready once started.
    // In the future, check if at least one node is registered or catalog is loaded.
    "OK"
}
