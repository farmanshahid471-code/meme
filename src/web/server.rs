//! Axum web server setup and configuration

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::http::HeaderValue;
use axum::middleware;
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

use super::auth::require_api_key;
use super::routes::create_routes;
use super::AppState;
use crate::config::Config;

/// Build the CORS layer from `CORS_ORIGINS`.
///
/// `*` (the historical default) keeps working but logs a warning: combined with
/// an unset `API_KEY` it means any web page can drive the bot.
fn build_cors(config: &Config) -> CorsLayer {
    let base = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any);

    let origins: Vec<HeaderValue> = config
        .cors_origins
        .iter()
        .filter(|origin| origin.as_str() != "*")
        .filter_map(|origin| match origin.parse::<HeaderValue>() {
            Ok(value) => Some(value),
            Err(_) => {
                warn!("Ignoring invalid CORS origin in CORS_ORIGINS: {}", origin);
                None
            }
        })
        .collect();

    if origins.is_empty() || config.cors_origins.iter().any(|o| o == "*") {
        warn!(
            "CORS_ORIGINS is '*' — every web page can call this API. \
             Set CORS_ORIGINS to your dashboard URL in production."
        );
        base.allow_origin(Any)
    } else {
        info!("CORS restricted to {} origin(s)", origins.len());
        base.allow_origin(origins)
    }
}

/// Build the fully layered router (CORS outermost, then auth, then routes).
fn build_router(state: AppState, config: &Config) -> Router {
    if config.api_key.is_none() {
        warn!(
            "API_KEY is not set — the trading API is UNAUTHENTICATED. \
             Anyone who can reach this port can start/stop trading. \
             Set API_KEY=<long-random-string> and send it as the X-API-Key header."
        );
    } else {
        info!("🔐 API key authentication enabled");
    }

    create_routes(state.clone())
        .layer(middleware::from_fn_with_state(state, require_api_key))
        .layer(build_cors(config))
        .layer(TraceLayer::new_for_http())
}

/// Start the Axum web server
pub async fn start_server(state: AppState, config: Arc<Config>) -> Result<()> {
    let app = build_router(state, &config);

    // Determine bind address
    let host = config.api_host.as_deref().unwrap_or("0.0.0.0");
    let port = config.api_port.unwrap_or(3000);
    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .context("Invalid API_HOST or API_PORT")?;

    info!("Starting API server on http://{}", addr);

    // Start the server
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("Failed to bind to address")?;

    axum::serve(listener, app)
        .await
        .context("Server error")?;

    Ok(())
}

/// Create the Axum router without starting the server (useful for testing)
pub fn create_app(state: AppState) -> Router {
    let config = state.config.clone();
    build_router(state, &config)
}
