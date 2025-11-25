use anyhow::Result;
use redb::{Database, ReadableTable, TableDefinition};
use std::sync::Arc;
use std::{
    fs,
    path::{Path, PathBuf},
};

// Table Definitions
const BLOCKS: TableDefinition<u64, &str> = TableDefinition::new("blocks");
const INSCRIPTIONS: TableDefinition<&str, &str> = TableDefinition::new("inscriptions");
const TOKENS: TableDefinition<&str, &str> = TableDefinition::new("tokens");

// BRC-20 compliant balance tracking: address:ticker -> (available, overall)
const BALANCES: TableDefinition<&str, &str> = TableDefinition::new("balances");

// Transfer inscriptions: inscription_id -> transfer_data
const TRANSFER_INSCRIPTIONS: TableDefinition<&str, &str> =
    TableDefinition::new("transfer_inscriptions");

// Inscription number mapping: number -> inscription_id
const INSCRIPTION_NUMBERS: TableDefinition<u64, &str> = TableDefinition::new("inscription_numbers");
// Address index: address -> json_list_of_inscription_ids
const ADDRESS_INSCRIPTIONS: TableDefinition<&str, &str> =
    TableDefinition::new("address_inscriptions");
// Inscription state: inscription_id -> current_owner
const INSCRIPTION_STATE: TableDefinition<&str, &str> = TableDefinition::new("inscription_state");
// Global stats
const STATS: TableDefinition<&str, u64> = TableDefinition::new("stats");

// Names: name -> name_data (ZNS - Zcash Name Service)
const NAMES: TableDefinition<&str, &str> = TableDefinition::new("names");

#[derive(Clone)]
/// Shared handle to the redb-backed state store.
pub struct Db {
    db: Arc<Database>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Balance {
    pub available: u64,
    pub overall: u64,
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

            // Update global count and number mapping
            let mut stats = write_txn.open_table(STATS)?;
            let count = stats
                .get("inscription_count")?
                .map(|v| v.value())
                .unwrap_or(0);
            let number = count + 1;
            stats.insert("inscription_count", number)?;

            let mut numbers = write_txn.open_table(INSCRIPTION_NUMBERS)?;
            numbers.insert(number, id)?;

            // Parse data to get address (simplified)
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
                // Also index receiver if different? For now just sender/owner.
                // Ideally we track ownership changes, but this is a start.
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

    pub fn get_token_info(&self, ticker: &str) -> Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TOKENS)?;
        let val = table.get(ticker)?.map(|v| v.value().to_string());
        Ok(val)
    }

    pub fn update_token_supply(&self, ticker: &str, new_supply: u64) -> Result<()> {
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

    // BRC-20 compliant balance operations
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
        available_delta: i64,
        overall_delta: i64,
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

            // Check for underflow
            if available_delta < 0 && current.available < (-available_delta as u64) {
                return Err(anyhow::anyhow!("Insufficient available balance"));
            }
            if overall_delta < 0 && current.overall < (-overall_delta as u64) {
                return Err(anyhow::anyhow!("Insufficient overall balance"));
            }

            let new_balance = Balance {
                available: if available_delta >= 0 {
                    current.available + (available_delta as u64)
                } else {
                    current.available - (-available_delta as u64)
                },
                overall: if overall_delta >= 0 {
                    current.overall + (overall_delta as u64)
                } else {
                    current.overall - (-overall_delta as u64)
                },
            };

            table.insert(key.as_str(), serde_json::to_string(&new_balance)?.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    // Transfer inscription operations
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

    // Name (ZNS) operations
    pub fn register_name(&self, name: &str, data: &str) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(NAMES)?;
            // Check if already exists (first-is-first)
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
