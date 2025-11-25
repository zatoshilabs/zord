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
    // Initialize logging
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

    // Config
    let db_path = env::var("DB_PATH").unwrap_or("./data/index".to_string());
    let api_port = env::var("API_PORT")
        .or_else(|_| env::var("PORT"))
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()?;

    // Components
    let db = db::Db::new(&db_path)?;
    let rpc = rpc::ZcashRpcClient::new();
    let indexer = indexer::Indexer::new(rpc, db.clone());

    // Spawn Indexer
    let indexer_handle = tokio::spawn(async move {
        if let Err(e) = indexer.start().await {
            tracing::error!("Indexer failed: {}", e);
        }
    });

    // Run API
    tracing::info!("Starting API on port {}", api_port);
    api::start_api(db, api_port).await;

    // Wait for indexer (though API will block main)
    let _ = indexer_handle.await;

    Ok(())
}
