//! HTTP application: composes request parsing, the Source registry, the
//! image processor, response headers, JSON error bodies, and completion
//! logging.

use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::config::AppConfig;
use crate::sources::SourceRegistry;

/// Shared state for the HTTP application.
pub struct AppState {
    pub config: AppConfig,
    pub registry: SourceRegistry,
    /// Process-wide derivation permits: no more than
    /// `max_concurrent_derivations` Source Objects are fetched or processed
    /// at once. Acquired before fetching, held through processing.
    pub derivation_permits: Semaphore,
}

impl AppState {
    pub fn new(config: AppConfig, registry: SourceRegistry) -> Arc<Self> {
        let permits = config.max_concurrent_derivations;
        Arc::new(AppState {
            config,
            registry,
            derivation_permits: Semaphore::new(permits),
        })
    }
}

/// Build the router serving the image contract.
pub fn build_router(state: Arc<AppState>) -> axum::Router {
    let _ = state;
    todo!("implemented by the HTTP application module")
}

/// Bind `config.listen_address` and serve until shutdown.
pub async fn run(config: AppConfig) -> Result<(), Box<dyn std::error::Error>> {
    let _ = config;
    todo!("implemented by the HTTP application module")
}
