use crate::db::Db;
use crate::names::NamesEngine;
use crate::rpc::ZcashRpcClient;
use crate::zrc20::Zrc20Engine;
use anyhow::Result;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;

pub struct Indexer {
    rpc: ZcashRpcClient,
    db: Db,
    zrc20: Zrc20Engine,
    names: NamesEngine,
}

impl Indexer {
    pub fn new(rpc: ZcashRpcClient, db: Db) -> Self {
        let zrc20 = Zrc20Engine::new(db.clone());
        let names = NamesEngine::new(db.clone());
        Self {
            rpc,
            db,
            zrc20,
            names,
        }
    }

    pub async fn start(&self) -> Result<()> {
        let start_height = std::env::var("ZSTART_HEIGHT")
            .unwrap_or("3132356".to_string())
            .parse::<u64>()?;

        let zmq_url = std::env::var("ZMQ_URL").ok();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);

        if let Some(url) = zmq_url {
            tracing::info!("Starting ZMQ listener on {}", url);
            crate::zmq::ZmqListener::new(url, tx).start();
        } else {
            tracing::warn!("ZMQ_URL not set, falling back to polling only");
        }

        loop {
            let current_height = self
                .db
                .get_latest_indexed_height()?
                .unwrap_or(start_height - 1);
            let chain_height = self.rpc.get_block_count().await?;

            if current_height < chain_height {
                let next_height = current_height + 1;
                match self.index_block(next_height).await {
                    Ok(_) => {
                        tracing::info!("Indexed block {}", next_height);
                    }
                    Err(e) => {
                        tracing::error!("Error indexing block {}: {}", next_height, e);
                        sleep(Duration::from_secs(5)).await;
                    }
                }
            } else {
                // We are at the tip. Wait for ZMQ notification or timeout.
                tokio::select! {
                    _ = rx.recv() => {
                        tracing::debug!("Received ZMQ block notification");
                        // Loop immediately to check for new block
                    }
                    _ = sleep(Duration::from_secs(10)) => {
                        // Poll fallback
                    }
                }
            }
        }
    }

    async fn index_block(&self, height: u64) -> Result<()> {
        let hash = self.rpc.get_block_hash(height).await?;
        let block = self.rpc.get_block(&hash).await?;

        // Track inscriptions created in this block
        let mut inscriptions_in_block: HashMap<String, (String, String)> = HashMap::new();

        // First pass: Find all new inscriptions (inscribe events)
        for txid in &block.tx {
            let tx = self.rpc.get_raw_transaction(&txid).await?;

            // Scan inputs for inscriptions (Ordinals-style in scriptSig)
            for (_vin_index, vin) in tx.vin.iter().enumerate() {
                if let Some(script_sig) = &vin.script_sig {
                    if let Some(inscription) = self.parse_inscription(&script_sig.asm, &txid, &tx) {
                        let inscription_id = inscription.0;
                        let sender = inscription.1;
                        let receiver = inscription.2;
                        let content_type = inscription.3;
                        let content = inscription.4;
                        let content_hex = inscription.5;

                        // Store for tracking
                        inscriptions_in_block
                            .insert(inscription_id.clone(), (sender.clone(), content.clone()));

                        // Save inscription to DB with content type and hex
                        // Persist all descriptive fields so the HTTP layer can serve them without
                        // rehydrating the block/transaction.
                        let metadata = serde_json::json!({
                            "id": inscription_id,
                            "content": content,
                            "content_hex": content_hex,
                            "content_type": content_type,
                            "txid": txid,
                            "vout": 0,
                            "sender": sender,
                            "receiver": receiver,
                            "block_height": height,
                            "block_time": block.time,
                        });

                        self.db
                            .insert_inscription(&inscription_id, &metadata.to_string())?;

                        // Log based on content type
                        if content_type == "application/json" {
                            tracing::info!(
                                "Found JSON inscription {} in block {}: {}",
                                inscription_id,
                                height,
                                content
                            );
                        } else if content_type.starts_with("text/") {
                            let preview = if content.len() > 100 {
                                format!("{}...", &content[..100])
                            } else {
                                content.clone()
                            };
                            tracing::info!(
                                "Found text inscription {} in block {} ({}): {}",
                                inscription_id,
                                height,
                                content_type,
                                preview
                            );
                        } else {
                            tracing::info!(
                                "Found inscription {} in block {} ({}): {} bytes",
                                inscription_id,
                                height,
                                content_type,
                                content_hex.len() / 2
                            );
                        }

                        // Process ZRC-20 if it's JSON
                        if content_type == "application/json" {
                            if let Err(e) = self.zrc20.process(
                                "inscribe",
                                &inscription_id,
                                &sender,
                                Some(&receiver),
                                &content,
                            ) {
                                tracing::debug!("Not a valid ZRC-20 operation: {}", e);
                            }
                        }

                        // Process names if it's plain text
                        if content_type == "text/plain" {
                            if let Err(e) = self.names.process(
                                &inscription_id,
                                &sender,
                                &content,
                                &content_type,
                            ) {
                                tracing::debug!("Not a valid name registration: {}", e);
                            }
                        }
                    }
                }
            }
        }

        // Second pass: Detect transfer of existing inscriptions
        // In a real implementation, we would track UTXO movements of inscriptions
        // For MVP, we'll detect when an inscription appears in a different transaction
        // This is simplified; a production indexer would need full UTXO tracking

        // TODO: Implement inscription transfer detection
        // This requires tracking which UTXOs contain inscriptions and detecting when they move
        // For now, we'll focus on the inscribe events

        self.db.insert_block(height, &hash)?;
        Ok(())
    }

    /// Parse inscription from scriptSig ASM
    /// Returns: (inscription_id, sender, receiver, content_type, content_utf8, content_hex)
    fn parse_inscription(
        &self,
        asm: &str,
        txid: &str,
        tx: &crate::rpc::TxResponse,
    ) -> Option<(String, String, String, String, String, String)> {
        let parts: Vec<&str> = asm.split_whitespace().collect();

        // Zerdinals uses a simpler format than Bitcoin Ordinals (no Taproot on Zcash)
        // Look for content-type followed by content data in scriptSig
        // Pattern is more flexible: <content_type_hex> <content_data_hex>...
        for i in 0..parts.len() {
            // Try to decode as potential content type (should contain "/")
            if let Ok(bytes) = hex::decode(parts[i]) {
                if let Ok(s) = String::from_utf8(bytes) {
                    // Check if this looks like a MIME content type
                    if s.contains("/") && s.len() > 3 && s.len() < 100 {
                        let content_type = s;

                        // Collect all following hex data chunks until sig/pubkey
                        let mut content_chunks = Vec::new();
                        let mut j = i + 1;

                        while j < parts.len() {
                            let part = parts[j];

                            // Skip small OP codes (1-2 chars like 0, 1, OP_codes)
                            if part.len() <= 2 {
                                j += 1;
                                continue;
                            }

                            // Try to decode as hex data
                            if let Ok(data) = hex::decode(part) {
                                // We're near the end if within last 3 positions
                                let near_end = j >= parts.len() - 3;

                                // Check for signature (starts with 0x30, DER format)
                                let is_signature = data.len() >= 70
                                    && data.len() <= 74
                                    && data.get(0) == Some(&0x30);

                                // Check for pubkey patterns:
                                // - Exact 33 or 65 bytes (standard pubkey sizes)
                                // - Starts with 0x02/0x03 (compressed) or 0x04 (uncompressed)
                                // - Or starts with 0x21 (OP_PUSH 33 bytes) followed by pubkey
                                let is_pubkey = (data.len() == 33
                                    && (data.get(0) == Some(&0x02) || data.get(0) == Some(&0x03)))
                                    || (data.len() == 65 && data.get(0) == Some(&0x04))
                                    || (data.get(0) == Some(&0x21) && data.len() >= 34); // 0x21 = PUSH 33 bytes

                                // Skip if it looks like a signature or pubkey, especially near the end
                                if near_end && (is_signature || is_pubkey) {
                                    // Stop collecting content
                                    break;
                                }

                                // Otherwise, collect the content
                                if data.len() > 0 {
                                    content_chunks.push(data);
                                }
                            }

                            j += 1;
                        }

                        if content_chunks.is_empty() {
                            continue;
                        }

                        // Concatenate all chunks
                        let content_bytes: Vec<u8> = content_chunks.into_iter().flatten().collect();
                        let content_hex = hex::encode(&content_bytes);

                        // Try to decode as UTF-8 for text content types
                        let content_utf8 = if content_type.starts_with("text/")
                            || content_type == "application/json"
                        {
                            String::from_utf8(content_bytes.clone())
                                .unwrap_or_else(|_| content_hex.clone())
                        } else {
                            content_hex.clone()
                        };

                        // Get sender/receiver from first vout
                        let sender = if let Some(first_vout) = tx.vout.first() {
                            if let Some(addrs) = &first_vout.script_pub_key.addresses {
                                addrs
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| "unknown".to_string())
                            } else {
                                "unknown".to_string()
                            }
                        } else {
                            "unknown".to_string()
                        };

                        let receiver = sender.clone();
                        let inscription_id = format!("{}i0", txid);

                        tracing::info!(
                            "Found inscription {} with content type: {} ({} bytes)",
                            inscription_id,
                            content_type,
                            content_bytes.len()
                        );

                        return Some((
                            inscription_id,
                            sender,
                            receiver,
                            content_type,
                            content_utf8,
                            content_hex,
                        ));
                    }
                }
            }
        }

        None
    }
}
