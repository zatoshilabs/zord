use anyhow::Result;
use redb::{Database, ReadableTable, TableDefinition};
use std::sync::Arc;
use std::{
    fs,
    path::{Path, PathBuf},
};

// redb table schemas
const BLOCKS: TableDefinition<u64, &str> = TableDefinition::new("blocks");
const INSCRIPTIONS: TableDefinition<&str, &str> = TableDefinition::new("inscriptions");
const TOKENS: TableDefinition<&str, &str> = TableDefinition::new("tokens");

// Balance table keyed by "address:ticker"
const BALANCES: TableDefinition<&str, &str> = TableDefinition::new("balances");

// Pending transfer metadata keyed by inscription id
const TRANSFER_INSCRIPTIONS: TableDefinition<&str, &str> =
    TableDefinition::new("transfer_inscriptions");
// Map outpoint ("<txid>:<vout>") -> transfer inscription id
const TRANSFER_OUTPOINTS: TableDefinition<&str, &str> =
    TableDefinition::new("transfer_outpoints");

// Ordinal number -> inscription id mapping
const INSCRIPTION_NUMBERS: TableDefinition<u64, &str> = TableDefinition::new("inscription_numbers");
// Address index contains a JSON list of inscription ids
const ADDRESS_INSCRIPTIONS: TableDefinition<&str, &str> =
    TableDefinition::new("address_inscriptions");
// Latest owner map for quick lookups
const INSCRIPTION_STATE: TableDefinition<&str, &str> = TableDefinition::new("inscription_state");
// Simple aggregate counters and status values
const STATS: TableDefinition<&str, u64> = TableDefinition::new("stats");
const STATUS: TableDefinition<&str, u64> = TableDefinition::new("status");

// ZNS backing store
const NAMES: TableDefinition<&str, &str> = TableDefinition::new("names");
const ZRC721_COLLECTIONS: TableDefinition<&str, &str> =
    TableDefinition::new("zrc721_collections");
const ZRC721_TOKENS: TableDefinition<&str, &str> = TableDefinition::new("zrc721_tokens");

#[derive(Clone)]
/// Shared handle to the redb-backed state store.
pub struct Db {
    db: Arc<Database>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Balance {
    pub available: u128,
    pub overall: u128,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Zrc721Token {
    pub tick: String,
    pub token_id: String,
    pub owner: String,
    pub inscription_id: String,
    pub metadata: serde_json::Value,
}

impl Db {
    pub fn new(path: impl AsRef<Path>, reindex: bool) -> Result<Self> {
        let path = PathBuf::from(path.as_ref());
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        if reindex && path.exists() {
            tracing::warn!("RE_INDEX=TRUE deleting db at {:?}", path);
            fs::remove_file(&path)?;
        }

        let db = Database::create(&path)?;

        let write_txn = db.begin_write()?;
        {
            write_txn.open_table(BLOCKS)?;
            write_txn.open_table(INSCRIPTIONS)?;
            write_txn.open_table(TOKENS)?;
            write_txn.open_table(BALANCES)?;
            write_txn.open_table(TRANSFER_INSCRIPTIONS)?;
            write_txn.open_table(TRANSFER_OUTPOINTS)?;
            write_txn.open_table(INSCRIPTION_STATE)?;
            write_txn.open_table(INSCRIPTION_NUMBERS)?;
            write_txn.open_table(ADDRESS_INSCRIPTIONS)?;
            write_txn.open_table(STATS)?;
            write_txn.open_table(STATUS)?;
            write_txn.open_table(NAMES)?;
            write_txn.open_table(ZRC721_COLLECTIONS)?;
            write_txn.open_table(ZRC721_TOKENS)?;
        }
        write_txn.commit()?;

        Ok(Self { db: Arc::new(db) })
    }

    pub fn get_latest_indexed_height(&self) -> Result<Option<u64>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(BLOCKS)?;
        let result = match table.last()? {
            Some((k, _)) => Some(k.value()),
            None => None,
        };
        Ok(result)
    }

    pub fn insert_block(&self, height: u64, hash: &str) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(BLOCKS)?;
            table.insert(height, hash)?;

            let mut status = write_txn.open_table(STATUS)?;
            status.insert("core_height", height)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn insert_inscription(&self, id: &str, data: &str) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(INSCRIPTIONS)?;
            table.insert(id, data)?;

            // Maintain monotonic inscription numbering for API lookups
            let mut stats = write_txn.open_table(STATS)?;
            let count = stats
                .get("inscription_count")?
                .map(|v| v.value())
                .unwrap_or(0);
            let number = count + 1;
            stats.insert("inscription_count", number)?;

            let mut numbers = write_txn.open_table(INSCRIPTION_NUMBERS)?;
            numbers.insert(number, id)?;

            // Index sender so `/address/:addr/inscriptions` can return results
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                if let Some(sender) = json["sender"].as_str() {
                    let mut addr_index = write_txn.open_table(ADDRESS_INSCRIPTIONS)?;
                    let mut list = if let Some(existing) = addr_index.get(sender)? {
                        serde_json::from_str::<Vec<String>>(existing.value()).unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    list.push(id.to_string());
                    addr_index.insert(sender, serde_json::to_string(&list)?.as_str())?;
                }
                // Receiver tracking is future work; today we key by sender only
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_inscriptions_page(
        &self,
        page: usize,
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        let offset = page.saturating_mul(limit);
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(INSCRIPTIONS)?;
        let mut items = Vec::new();

        for item in table.iter()?.rev().skip(offset).take(limit) {
            let (k, v) = item?;
            items.push((k.value().to_string(), v.value().to_string()));
        }

        Ok(items)
    }

    // Token operations
    pub fn deploy_token(&self, ticker: &str, info: &str) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TOKENS)?;
            if table.get(ticker)?.is_some() {
                return Err(anyhow::anyhow!("Token already exists"));
            }
            table.insert(ticker, info)?;

            let mut stats = write_txn.open_table(STATS)?;
            let count = stats.get("token_count")?.map(|v| v.value()).unwrap_or(0);
            stats.insert("token_count", count + 1)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_tokens_page(&self, page: usize, limit: usize) -> Result<Vec<(String, String)>> {
        let offset = page.saturating_mul(limit);
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TOKENS)?;
        let mut tokens = Vec::new();
        for item in table.iter()?.rev().skip(offset).take(limit) {
            let (k, v) = item?;
            tokens.push((k.value().to_string(), v.value().to_string()));
        }
        Ok(tokens)
    }

    pub fn search_tokens(&self, query: &str, limit: usize) -> Result<Vec<(String, String)>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TOKENS)?;
        let mut tokens = Vec::new();
        // Case-insensitive scan (dataset is small enough for a linear walk)
        let query_lower = query.to_lowercase();
        for item in table.iter()? {
            let (k, v) = item?;
            let ticker = k.value();
            if ticker.to_lowercase().contains(&query_lower) {
                tokens.push((ticker.to_string(), v.value().to_string()));
                if tokens.len() >= limit {
                    break;
                }
            }
        }
        Ok(tokens)
    }

    pub fn get_token_info(&self, ticker: &str) -> Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TOKENS)?;
        let val = table.get(ticker)?.map(|v| v.value().to_string());
        Ok(val)
    }

    pub fn update_token_supply(&self, ticker: &str, new_supply: u128) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TOKENS)?;
            let info_str = table
                .get(ticker)?
                .ok_or(anyhow::anyhow!("Token not found"))?
                .value()
                .to_string();

            let mut info: serde_json::Value = serde_json::from_str(&info_str)?;
            info["supply"] = serde_json::Value::String(new_supply.to_string());
            table.insert(ticker, info.to_string().as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Atomically credit a mint: increase token supply and holder balance
    /// in a single write transaction to prevent supply/balance drift.
    pub fn mint_credit_atomic(&self, ticker: &str, address: &str, amt: u128) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            // Update token supply
            let mut tokens = write_txn.open_table(TOKENS)?;
            let info_str = tokens
                .get(ticker)?
                .ok_or(anyhow::anyhow!("Token not found"))?
                .value()
                .to_string();
            let mut info: serde_json::Value = serde_json::from_str(&info_str)?;
            let current_supply: u128 = info["supply"]
                .as_str()
                .and_then(|s| s.parse::<u128>().ok())
                .unwrap_or(0);
            let new_supply = current_supply
                .checked_add(amt)
                .ok_or_else(|| anyhow::anyhow!("Supply overflow"))?;
            info["supply"] = serde_json::Value::String(new_supply.to_string());
            tokens.insert(ticker, info.to_string().as_str())?;

            // Update holder balance (available and overall)
            let mut balances = write_txn.open_table(BALANCES)?;
            let key = format!("{}:{}", address, ticker);
            let current = if let Some(val) = balances.get(key.as_str())? {
                serde_json::from_str::<Balance>(val.value())?
            } else {
                Balance {
                    available: 0,
                    overall: 0,
                }
            };

            let next_available = (current.available as u128)
                .checked_add(amt)
                .ok_or_else(|| anyhow::anyhow!("Available balance overflow"))?;
            let next_overall = (current.overall as u128)
                .checked_add(amt)
                .ok_or_else(|| anyhow::anyhow!("Overall balance overflow"))?;

            let new_balance = Balance {
                available: next_available,
                overall: next_overall,
            };
            balances.insert(key.as_str(), serde_json::to_string(&new_balance)?.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    // Balance helpers (available vs overall mirrors BRC-20 semantics)
    pub fn get_balance(&self, address: &str, ticker: &str) -> Result<Balance> {
        let key = format!("{}:{}", address, ticker);
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(BALANCES)?;

        let balance = if let Some(val) = table.get(key.as_str())? {
            serde_json::from_str::<Balance>(val.value())?
        } else {
            Balance {
                available: 0,
                overall: 0,
            }
        };
        Ok(balance)
    }

    pub fn update_balance(
        &self,
        address: &str,
        ticker: &str,
        available_delta: i128,
        overall_delta: i128,
    ) -> Result<()> {
        let key = format!("{}:{}", address, ticker);
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(BALANCES)?;
            let current = if let Some(val) = table.get(key.as_str())? {
                serde_json::from_str::<Balance>(val.value())?
            } else {
                Balance {
                    available: 0,
                    overall: 0,
                }
            };

            let next_available = (current.available as i128)
                .checked_add(available_delta)
                .ok_or_else(|| anyhow::anyhow!("Available balance overflow"))?;
            if next_available < 0 {
                return Err(anyhow::anyhow!("Insufficient available balance"));
            }

            let next_overall = (current.overall as i128)
                .checked_add(overall_delta)
                .ok_or_else(|| anyhow::anyhow!("Overall balance overflow"))?;
            if next_overall < 0 {
                return Err(anyhow::anyhow!("Insufficient overall balance"));
            }

            let new_balance = Balance {
                available: next_available as u128,
                overall: next_overall as u128,
            };

            table.insert(key.as_str(), serde_json::to_string(&new_balance)?.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn list_balances_for_tick(
        &self,
        tick: &str,
        page: usize,
        limit: usize,
    ) -> Result<(Vec<(String, Balance)>, usize)> {
        let needle = tick.to_lowercase();
        let offset = page.saturating_mul(limit);
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(BALANCES)?;
        let mut rows = Vec::new();
        for item in table.iter()? {
            let (k, v) = item?;
            let key = k.value();
            if let Some((address, token)) = key.split_once(':') {
                if token == needle {
                    let bal = serde_json::from_str::<Balance>(v.value())?;
                    rows.push((address.to_string(), bal));
                }
            }
        }
        rows.sort_by(|a, b| b.1.overall.cmp(&a.1.overall));
        let total = rows.len();
        let page_rows = rows.into_iter().skip(offset).take(limit).collect();
        Ok((page_rows, total))
    }

    /// Sum balances for a given ticker across all addresses.
    /// Returns (sum_overall, sum_available, holder_count).
    pub fn sum_balances_for_tick(&self, tick: &str) -> Result<(u128, u128, usize)> {
        let needle = tick.to_lowercase();
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(BALANCES)?;
        let mut sum_overall: u128 = 0;
        let mut sum_available: u128 = 0;
        let mut count: usize = 0;
        for item in table.iter()? {
            let (k, v) = item?;
            let key = k.value();
            if let Some((_address, token)) = key.split_once(':') {
                if token == needle {
                    let bal = serde_json::from_str::<Balance>(v.value())?;
                    sum_overall = sum_overall
                        .checked_add(bal.overall)
                        .ok_or_else(|| anyhow::anyhow!("overall sum overflow"))?;
                    sum_available = sum_available
                        .checked_add(bal.available)
                        .ok_or_else(|| anyhow::anyhow!("available sum overflow"))?;
                    count += 1;
                }
            }
        }
        Ok((sum_overall, sum_available, count))
    }

    /// Count completed (settled) transfer inscriptions for a given ticker.
    pub fn count_completed_transfers_for_tick(&self, tick: &str) -> Result<u64> {
        let needle = tick.to_lowercase();
        let read_txn = self.db.begin_read()?;
        let transfers = read_txn.open_table(TRANSFER_INSCRIPTIONS)?;
        let state = read_txn.open_table(INSCRIPTION_STATE)?;
        let mut count: u64 = 0;
        for item in transfers.iter()? {
            let (k, v) = item?;
            // parse transfer payload and match ticker
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(v.value()) {
                if val["tick"].as_str().map(|s| s == needle).unwrap_or(false) {
                    let id = k.value();
                    if let Some(st) = state.get(id)? {
                        if st.value() == "used" {
                            count += 1;
                        }
                    }
                }
            }
        }
        Ok(count)
    }

    /// Compute rank (1-based) and total holders for a ticker by overall balance.
    /// Returns (rank, total_holders). If address not found or has zero, rank is null (0).
    pub fn rank_for_address_in_tick(&self, tick: &str, address: &str) -> Result<(u64, u64)> {
        let needle = tick.to_lowercase();
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(BALANCES)?;
        let mut rows: Vec<(String, u128)> = Vec::new();
        for item in table.iter()? {
            let (k, v) = item?;
            if let Some((addr, token)) = k.value().split_once(':') {
                if token == needle {
                    let bal = serde_json::from_str::<Balance>(v.value())?;
                    if bal.overall > 0 {
                        rows.push((addr.to_string(), bal.overall));
                    }
                }
            }
        }
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        let total = rows.len() as u64;
        let mut rank: u64 = 0;
        for (idx, (addr, _)) in rows.iter().enumerate() {
            if addr == address {
                rank = (idx as u64) + 1;
                break;
            }
        }
        Ok((rank, total))
    }

    pub fn list_balances_for_address(&self, address: &str) -> Result<Vec<(String, Balance)>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(BALANCES)?;
        let mut rows = Vec::new();
        for item in table.iter()? {
            let (k, v) = item?;
            let key = k.value();
            if let Some((addr, token)) = key.split_once(':') {
                if addr == address {
                    let bal = serde_json::from_str::<Balance>(v.value())?;
                    rows.push((token.to_string(), bal));
                }
            }
        }
        rows.sort_by(|a, b| b.1.overall.cmp(&a.1.overall));
        Ok(rows)
    }

    pub fn set_status(&self, key: &str, value: u64) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(STATUS)?;
            table.insert(key, value)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_status(&self, key: &str) -> Result<Option<u64>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(STATUS)?;
        let value = table.get(key)?.map(|v| v.value());
        Ok(value)
    }

    pub fn register_zrc721_collection(
        &self,
        tick: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(ZRC721_COLLECTIONS)?;
            if table.get(tick)?.is_some() {
                return Err(anyhow::anyhow!("Collection already exists"));
            }
            table.insert(tick, payload.to_string().as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_zrc721_collection(&self, tick: &str) -> Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(ZRC721_COLLECTIONS)?;
        let val = table.get(tick)?.map(|v| v.value().to_string());
        Ok(val)
    }

    pub fn list_zrc721_collections(&self, page: usize, limit: usize) -> Result<Vec<(String, String)>> {
        let offset = page.saturating_mul(limit);
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(ZRC721_COLLECTIONS)?;
        let mut rows = Vec::new();
        for item in table.iter()?.rev().skip(offset).take(limit) {
            let (k, v) = item?;
            rows.push((k.value().to_string(), v.value().to_string()));
        }
        Ok(rows)
    }

    pub fn insert_zrc721_token(
        &self,
        tick: &str,
        token_id: &str,
        owner: &str,
        inscription_id: &str,
        metadata: &serde_json::Value,
    ) -> Result<()> {
        let key = format!("{}#{}", tick, token_id);
        let write_txn = self.db.begin_write()?;
        {
            let mut collections = write_txn.open_table(ZRC721_COLLECTIONS)?;
            let mut tokens = write_txn.open_table(ZRC721_TOKENS)?;

            if tokens.get(key.as_str())?.is_some() {
                return Err(anyhow::anyhow!("Token already minted"));
            }

            let mut collection: serde_json::Value = match collections.get(tick)? {
                Some(raw) => serde_json::from_str(raw.value())?,
                None => return Err(anyhow::anyhow!("Collection not found")),
            };
            // Enforce supply-based cap and token id range (0..=supply-1)
            let current_minted = collection["minted"].as_u64().unwrap_or(0);
            let max_allowed = collection["supply"].as_str().and_then(|s| s.parse::<u64>().ok());
            if let Some(max_total) = max_allowed {
                if current_minted >= max_total {
                    return Err(anyhow::anyhow!("Max token count reached"));
                }
                if let Ok(id_num) = token_id.parse::<u64>() {
                    if id_num >= max_total {
                        return Err(anyhow::anyhow!("Token id out of range"));
                    }
                }
            }
            let minted = current_minted + 1;
            collection["minted"] = serde_json::json!(minted);
            collections.insert(tick, collection.to_string().as_str())?;

            let token = Zrc721Token {
                tick: tick.to_string(),
                token_id: token_id.to_string(),
                owner: owner.to_string(),
                inscription_id: inscription_id.to_string(),
                metadata: metadata.clone(),
            };
            tokens.insert(key.as_str(), serde_json::to_string(&token)?.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn list_zrc721_tokens(
        &self,
        tick: &str,
        page: usize,
        limit: usize,
    ) -> Result<Vec<Zrc721Token>> {
        let offset = page.saturating_mul(limit);
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(ZRC721_TOKENS)?;
        let mut rows = Vec::new();
        for item in table.iter()? {
            let (k, v) = item?;
            let key = k.value();
            if let Some((collection, _token)) = key.split_once('#') {
                if collection == tick {
                    let data: Zrc721Token = serde_json::from_str(v.value())?;
                    rows.push(data);
                }
            }
        }
        rows.sort_by(|a, b| a.token_id.cmp(&b.token_id));
        Ok(rows.into_iter().skip(offset).take(limit).collect())
    }

    pub fn list_zrc721_tokens_by_address(
        &self,
        address: &str,
        page: usize,
        limit: usize,
    ) -> Result<Vec<Zrc721Token>> {
        let offset = page.saturating_mul(limit);
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(ZRC721_TOKENS)?;
        let mut rows = Vec::new();
        for item in table.iter()? {
            let (_k, v) = item?;
            let data: Zrc721Token = serde_json::from_str(v.value())?;
            if data.owner == address {
                rows.push(data);
            }
        }
        rows.sort_by(|a, b| a.tick.cmp(&b.tick).then(a.token_id.cmp(&b.token_id)));
        Ok(rows.into_iter().skip(offset).take(limit).collect())
    }

    pub fn zrc721_counts(&self) -> Result<(usize, usize)> {
        let read_txn = self.db.begin_read()?;
        let collections = read_txn.open_table(ZRC721_COLLECTIONS)?;
        let tokens = read_txn.open_table(ZRC721_TOKENS)?;
        let collection_count = collections.len()? as usize;
        let token_count = tokens.len()? as usize;
        Ok((collection_count, token_count))
    }

    // Transfer inscription helpers
    pub fn create_transfer_inscription(&self, inscription_id: &str, data: &str) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TRANSFER_INSCRIPTIONS)?;
            table.insert(inscription_id, data)?;

            let mut state_table = write_txn.open_table(INSCRIPTION_STATE)?;
            state_table.insert(inscription_id, "unused")?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn register_transfer_outpoint(&self, txid: &str, vout: u32, inscription_id: &str) -> Result<()> {
        let key = format!("{}:{}", txid, vout);
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TRANSFER_OUTPOINTS)?;
            table.insert(key.as_str(), inscription_id)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_transfer_by_outpoint(&self, txid: &str, vout: u32) -> Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TRANSFER_OUTPOINTS)?;
        let key = format!("{}:{}", txid, vout);
        let val = table.get(key.as_str())?.map(|v| v.value().to_string());
        Ok(val)
    }

    pub fn remove_transfer_outpoint(&self, txid: &str, vout: u32) -> Result<()> {
        let key = format!("{}:{}", txid, vout);
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TRANSFER_OUTPOINTS)?;
            let _ = table.remove(key.as_str());
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Reverse lookup helper for debugging/APIs: find outpoint for a transfer inscription id.
    pub fn find_outpoint_by_transfer_id(&self, inscription_id: &str) -> Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TRANSFER_OUTPOINTS)?;
        for item in table.iter()? {
            let (k, v) = item?;
            if v.value() == inscription_id {
                return Ok(Some(k.value().to_string()));
            }
        }
        Ok(None)
    }

    pub fn get_transfer_inscription(&self, inscription_id: &str) -> Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TRANSFER_INSCRIPTIONS)?;
        let val = table.get(inscription_id)?.map(|v| v.value().to_string());
        Ok(val)
    }

    pub fn mark_inscription_used(&self, inscription_id: &str) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(INSCRIPTION_STATE)?;
            table.insert(inscription_id, "used")?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn is_inscription_used(&self, inscription_id: &str) -> Result<bool> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(INSCRIPTION_STATE)?;
        let val = table
            .get(inscription_id)?
            .map(|v| v.value() == "used")
            .unwrap_or(false);
        Ok(val)
    }

    pub fn get_inscription(&self, id: &str) -> Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(INSCRIPTIONS)?;
        let val = table.get(id)?.map(|v| v.value().to_string());
        Ok(val)
    }

    pub fn get_inscription_by_number(&self, number: u64) -> Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(INSCRIPTION_NUMBERS)?;
        let val = table.get(number)?.map(|v| v.value().to_string());
        Ok(val)
    }

    pub fn get_inscriptions_by_address(&self, address: &str) -> Result<Vec<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(ADDRESS_INSCRIPTIONS)?;
        let result = if let Some(val) = table.get(address)? {
            let list = serde_json::from_str::<Vec<String>>(val.value())?;
            list
        } else {
            Vec::new()
        };
        Ok(result)
    }

    pub fn get_all_tokens(&self) -> Result<Vec<(String, String)>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TOKENS)?;
        let mut tokens = Vec::new();
        for item in table.iter()? {
            let (k, v) = item?;
            tokens.push((k.value().to_string(), v.value().to_string()));
        }
        Ok(tokens)
    }

    pub fn get_inscription_count(&self) -> Result<u64> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(STATS)?;
        let count = table
            .get("inscription_count")?
            .map(|v| v.value())
            .unwrap_or(0);
        Ok(count)
    }

    // Name (ZNS) helpers
    pub fn register_name(&self, name: &str, data: &str) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(NAMES)?;
            // Enforce first-writer-wins
            if table.get(name)?.is_some() {
                return Err(anyhow::anyhow!("Name already registered"));
            }
            table.insert(name, data)?;

            let mut stats = write_txn.open_table(STATS)?;
            let count = stats.get("name_count")?.map(|v| v.value()).unwrap_or(0);
            stats.insert("name_count", count + 1)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_names_page(&self, page: usize, limit: usize) -> Result<Vec<(String, String)>> {
        let offset = page.saturating_mul(limit);
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(NAMES)?;
        let mut names = Vec::new();
        for item in table.iter()?.rev().skip(offset).take(limit) {
            let (k, v) = item?;
            names.push((k.value().to_string(), v.value().to_string()));
        }
        Ok(names)
    }

    pub fn search_names(&self, query: &str, limit: usize) -> Result<Vec<(String, String)>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(NAMES)?;
        let mut names = Vec::new();
        let query_lower = query.to_lowercase();
        
        // Case-insensitive scan; fine for the current data volume
        for item in table.iter()? {
            let (k, v) = item?;
            let name = k.value();
            if name.to_lowercase().contains(&query_lower) {
                names.push((name.to_string(), v.value().to_string()));
                if names.len() >= limit {
                    break;
                }
            }
        }
        Ok(names)
    }

    pub fn get_token_count(&self) -> Result<u64> {
        let read_txn = self.db.begin_read()?;
        let count;
        {
            let table = read_txn.open_table(STATS)?;
            count = table.get("token_count")?.map(|v| v.value()).unwrap_or(0);
        }
        Ok(count)
    }

    pub fn get_name_count(&self) -> Result<u64> {
        let read_txn = self.db.begin_read()?;
        let count;
        {
            let table = read_txn.open_table(STATS)?;
            count = table.get("name_count")?.map(|v| v.value()).unwrap_or(0);
        }
        Ok(count)
    }

    pub fn get_name(&self, name: &str) -> Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(NAMES)?;
        let val = table.get(name)?.map(|v| v.value().to_string());
        Ok(val)
    }

    pub fn get_all_names(&self) -> Result<Vec<(String, String)>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(NAMES)?;
        let mut names = Vec::new();
        for item in table.iter()? {
            let (k, v) = item?;
            names.push((k.value().to_string(), v.value().to_string()));
        }
        Ok(names)
    }
}
