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

// Ordinal number -> inscription id mapping
const INSCRIPTION_NUMBERS: TableDefinition<u64, &str> = TableDefinition::new("inscription_numbers");
// Address index contains a JSON list of inscription ids
const ADDRESS_INSCRIPTIONS: TableDefinition<&str, &str> =
    TableDefinition::new("address_inscriptions");
// Latest owner map for quick lookups
const INSCRIPTION_STATE: TableDefinition<&str, &str> = TableDefinition::new("inscription_state");
// Simple aggregate counters
const STATS: TableDefinition<&str, u64> = TableDefinition::new("stats");

// ZNS backing store
const NAMES: TableDefinition<&str, &str> = TableDefinition::new("names");

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

impl Db {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = PathBuf::from(path.as_ref());
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let db = Database::create(&path)?;

        let write_txn = db.begin_write()?;
        {
            write_txn.open_table(BLOCKS)?;
            write_txn.open_table(INSCRIPTIONS)?;
            write_txn.open_table(TOKENS)?;
            write_txn.open_table(BALANCES)?;
            write_txn.open_table(TRANSFER_INSCRIPTIONS)?;
            write_txn.open_table(INSCRIPTION_STATE)?;
            write_txn.open_table(INSCRIPTION_NUMBERS)?;
            write_txn.open_table(ADDRESS_INSCRIPTIONS)?;
            write_txn.open_table(STATS)?;
            write_txn.open_table(NAMES)?;
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
