use crate::db::Db;
use crate::names::NamesEngine;
use crate::rpc::{ScriptPubKey, ZcashRpcClient};
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
            let _ = self.db.set_status("chain_tip", chain_height);

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
                // Tip reached; block on ZMQ or fall back to a periodic poll
                tokio::select! {
                    _ = rx.recv() => {
                        tracing::debug!("Received ZMQ block notification");
                        // Wake the loop to pick up the new height
                    }
                    _ = sleep(Duration::from_secs(10)) => {
                        // Timer path for deployments without ZMQ
                    }
                }
            }
        }
    }

    async fn index_block(&self, height: u64) -> Result<()> {
        let hash = self.rpc.get_block_hash(height).await?;
        let block = self.rpc.get_block(&hash).await?;

        // Keep a map to correlate parent/child inscriptions if needed later
        let mut inscriptions_in_block: HashMap<String, (String, String)> = HashMap::new();

        // First pass: index every new inscription carried by the block
        for txid in &block.tx {
            let tx = self.rpc.get_raw_transaction(&txid).await?;

            // Zcash ordinals place the payload in scriptSig; walk each input
            for (_vin_index, vin) in tx.vin.iter().enumerate() {
                if let Some(script_sig) = &vin.script_sig {
                    if let Some(inscription) = self.parse_inscription(&script_sig.asm, &txid, &tx) {
                        let inscription_id = inscription.0;
                        let sender = inscription.1;
                        let receiver = inscription.2;
                        let content_type = inscription.3;
                        let content = inscription.4;
                        let content_hex = inscription.5;

                        // Track so later phases can link child inscriptions if required
                        inscriptions_in_block
                            .insert(inscription_id.clone(), (sender.clone(), content.clone()));

                        // Persist enough metadata for the HTTP layer to render without additional RPC calls
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

                        // Emit structured logs so ops can watch which payload types arrive
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

                        // JSON blobs may encode ZRC-20 ops; hand them to the engine
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

                        // Plain text payloads may be ZNS registrations
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

        // Transfer tracking is not implemented; full UTXO tracing will be required when
        // inscription ownership is needed beyond insert-time metadata

        self.db.insert_block(height, &hash)?;
        let _ = self.db.set_status("zrc20_height", height);
        let _ = self.db.set_status("names_height", height);
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

        // Zcash inscriptions embed "<mime-type-hex> <payload-hex> ..." in scriptSig
        for i in 0..parts.len() {
            // Interpret the part as UTF-8 and treat it as a MIME type if it looks sane
            if let Ok(bytes) = hex::decode(parts[i]) {
                if let Ok(s) = String::from_utf8(bytes) {
                    if s.contains("/") && s.len() > 3 && s.len() < 100 {
                        let content_type = s;

                        // Consume subsequent hex pushes until we hit what looks like sig/pubkey data
                        let mut content_chunks = Vec::new();
                        let mut j = i + 1;

                        while j < parts.len() {
                            let part = parts[j];

                            // Tiny tokens are usually opcodes; ignore them
                            if part.len() <= 2 {
                                j += 1;
                                continue;
                            }

                            if let Ok(data) = hex::decode(part) {
                                let near_end = j >= parts.len() - 3;

                                // DER signatures start with 0x30 and are ~70 bytes
                                let is_signature = data.len() >= 70
                                    && data.len() <= 74
                                    && data.get(0) == Some(&0x30);

                                // Pubkeys are either 33/65-byte blobs with the usual prefixes or
                                // an OP_PUSH marker followed by 33 bytes
                                let is_pubkey = (data.len() == 33
                                    && (data.get(0) == Some(&0x02) || data.get(0) == Some(&0x03)))
                                    || (data.len() == 65 && data.get(0) == Some(&0x04))
                                    || (data.get(0) == Some(&0x21) && data.len() >= 34); // 0x21 => push 33 bytes

                                // Stop accumulating once we bump into DER sigs or pubkeys near the end
                                if near_end && (is_signature || is_pubkey) {
                                    break;
                                }

                                if data.len() > 0 {
                                    content_chunks.push(data);
                                }
                            }

                            j += 1;
                        }

                        if content_chunks.is_empty() {
                            continue;
                        }

                        // Flatten collected chunks into a single buffer
                        let content_bytes: Vec<u8> = content_chunks.into_iter().flatten().collect();
                        let content_hex = hex::encode(&content_bytes);

                        // Keep UTF-8 for text/json payloads so higher layers get a preview
                        let content_utf8 = if content_type.starts_with("text/")
                            || content_type == "application/json"
                        {
                            String::from_utf8(content_bytes.clone())
                                .unwrap_or_else(|_| content_hex.clone())
                        } else {
                            content_hex.clone()
                        };

                        let (sender, _shielded) = tx
                            .vout
                            .first()
                            .map(|vout| classify_address(&vout.script_pub_key))
                            .unwrap_or_else(|| ("unknown".to_string(), false));

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

fn classify_address(script: &ScriptPubKey) -> (String, bool) {
    if let Some(addrs) = &script.addresses {
        if let Some(addr) = addrs.first() {
            return (addr.clone(), addr.starts_with('z'));
        }
    }
    ("unknown".to_string(), false)
}
