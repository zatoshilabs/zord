use crate::db::Db;
use crate::names::NamesEngine;
use crate::rpc::{ScriptPubKey, ZcashRpcClient};
use crate::zrc20::Zrc20Engine;
use crate::zrc721::Zrc721Engine;
use anyhow::Result;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;

pub struct Indexer {
    rpc: ZcashRpcClient,
    db: Db,
    zrc20: Zrc20Engine,
    names: NamesEngine,
    zrc721: Zrc721Engine,
}

impl Indexer {
    pub fn new(rpc: ZcashRpcClient, db: Db) -> Self {
        let zrc20 = Zrc20Engine::new(db.clone());
        let names = NamesEngine::new(db.clone());
        let zrc721 = Zrc721Engine::new(db.clone());
        Self {
            rpc,
            db,
            zrc20,
            names,
            zrc721,
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

            // Retry RPC calls with backoff to handle transient network errors
            let chain_height = match self.rpc.get_block_count().await {
                Ok(height) => height,
                Err(e) => {
                    tracing::warn!("Failed to get block count: {} - retrying in 10s", e);
                    sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };
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
                        // Pick an assigned vout for the inscription: prefer the first output with an address
                        // Prefer an output paying back to the sender; otherwise first address-bearing output
                        let mut assigned_vout: Option<u32> = None;
                        for o in &tx.vout {
                            if let Some(addrs) = &o.script_pub_key.addresses {
                                if addrs.iter().any(|a| a == &sender) {
                                    assigned_vout = Some(o.n);
                                    break;
                                }
                            }
                        }
                        if assigned_vout.is_none() {
                            assigned_vout = tx
                                .vout
                                .iter()
                                .find(|o| o.script_pub_key.addresses.as_ref().map(|a| !a.is_empty()).unwrap_or(false))
                                .map(|o| o.n);
                        }
                        let assigned_vout = assigned_vout.unwrap_or(0);

                        let metadata = serde_json::json!({
                            "id": inscription_id,
                            "content": content,
                            "content_hex": content_hex,
                            "content_type": content_type,
                            "txid": txid,
                            "vout": assigned_vout,
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

                        // Accept JSON payloads using robust MIME detection:
                        // - application/json
                        // - application/*+json (RFC 6839 structured suffix)
                        // - text/* when the body looks like JSON (starts with { or [)
                        // Case-insensitive, ignore parameters (e.g., "; charset=utf-8").
                        let looks_json = {
                            let s = content.trim_start();
                            s.starts_with('{') || s.starts_with('[')
                        };
                        let ct_simple = {
                            let lower = content_type.to_lowercase();
                            lower.split(';').next().unwrap_or("").trim().to_string()
                        };
                        let is_json_mime = ct_simple == "application/json" || ct_simple.ends_with("+json");
                        let is_text_like_json = ct_simple.starts_with("text/") && looks_json;
                        if is_json_mime || is_text_like_json {
                            if let Err(e) = self.zrc20.process(
                                "inscribe",
                                &inscription_id,
                                &sender,
                                Some(&receiver),
                                &content,
                                Some(txid),
                                Some(assigned_vout),
                            ) {
                                tracing::debug!("Not a valid ZRC-20 operation: {}", e);
                            }

                            if let Err(e) = self.zrc721.process(
                                "inscribe",
                                &inscription_id,
                                &sender,
                                &content,
                                Some(txid),
                                Some(assigned_vout),
                            ) {
                                tracing::debug!("Not a valid ZRC-721 operation: {}", e);
                            }
                        }

                        // Plain text payloads may be ZNS registrations
                        if ct_simple == "text/plain" && !looks_json {
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
            // After indexing inscriptions in this tx, scan inputs to detect transfer reveals
            for vin in &tx.vin {
                if let (Some(prev_txid), Some(prev_vout)) = (&vin.txid, vin.vout) {
                    if let Ok(Some(inscription_id)) = self.db.get_transfer_by_outpoint(prev_txid, prev_vout) {
                        // Heuristic receiver: first transparent address in current tx outputs
                        let mut receiver: Option<String> = None;
                        for out in &tx.vout {
                            if let Some(addrs) = &out.script_pub_key.addresses {
                                if let Some(first) = addrs.first() {
                                    receiver = Some(first.clone());
                                    break;
                                }
                            }
                        }

                        let _ = self.zrc20.settle_transfer(
                            &inscription_id,
                            receiver.as_deref(),
                        );
                        let _ = self.db.mark_inscription_used(&inscription_id);
                        let _ = self.db.remove_transfer_outpoint(prev_txid, prev_vout);
                        tracing::info!("Settled transfer reveal {} -> receiver {:?}", inscription_id, receiver);
                    }

                    // ZRC-721: ownership move if mint outpoint is spent
                    if let Ok(Some((collection, token_id))) = self.db.zrc721_by_outpoint(prev_txid, prev_vout) {
                        // Determine receiver: first transparent address in outputs; if none, mark shielded burn
                        let mut receiver: Option<String> = None;
                        let mut new_vout: Option<u32> = None;
                        for out in &tx.vout {
                            if let Some(addrs) = &out.script_pub_key.addresses {
                                if let Some(first) = addrs.first() {
                                    if !first.starts_with('z') {
                                        receiver = Some(first.clone());
                                        new_vout = Some(out.n);
                                        break;
                                    }
                                }
                            }
                        }
                        match (receiver, new_vout) {
                            (Some(addr), Some(vout)) => {
                                let _ = self.db.update_zrc721_owner(&collection, &token_id, &addr, false);
                                let _ = self.db.move_zrc721_outpoint(prev_txid, prev_vout, txid, vout);
                                tracing::info!("ZRC-721 moved: {}#{} -> {} (vout {})", collection, token_id, addr, vout);
                            }
                            _ => {
                                let _ = self.db.update_zrc721_owner(&collection, &token_id, "shielded", true);
                                // Remove outpoint mapping to prevent further attribution
                                let _ = self.db.move_zrc721_outpoint(prev_txid, prev_vout, txid, 0);
                                tracing::info!("ZRC-721 shielded burn: {}#{}", collection, token_id);
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
        let _ = self.db.set_status("zrc721_height", height);
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
