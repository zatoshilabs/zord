mod api;
mod db;
mod indexer;
mod names;
mod rpc;
mod zmq;
mod zrc20;

use anyhow::Result;
use std::env;
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<()> {
    // Logging setup
    let verbose = env::var("VERBOSE_LOGS")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);
    let max_level = if verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    let subscriber = FmtSubscriber::builder().with_max_level(max_level).finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    // Runtime configuration
    let db_path = env::var("DB_PATH").unwrap_or("./data/index".to_string());
    let api_port = env::var("API_PORT")
        .or_else(|_| env::var("PORT"))
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()?;

    // Construct core services
    let reindex = env::var("RE_INDEX")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);
    let db = db::Db::new(&db_path, reindex)?;
    let rpc = rpc::ZcashRpcClient::new();
    let indexer = indexer::Indexer::new(rpc, db.clone());

    // Indexer runs alongside the HTTP server
    let indexer_handle = tokio::spawn(async move {
        if let Err(e) = indexer.start().await {
            tracing::error!("Indexer failed: {}", e);
        }
    });

    // Start the public API
    tracing::info!("Starting API on port {}", api_port);
    api::start_api(db, api_port).await;

    // Keep process alive even if API finishes unexpectedly
    let _ = indexer_handle.await;

    Ok(())
}
