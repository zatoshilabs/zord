use crate::db::Db;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Zrc20Operation {
    pub p: String,
    pub op: String,
    pub tick: String,
    #[serde(default)]
    pub max: Option<String>,
    #[serde(default)]
    pub lim: Option<String>,
    #[serde(default)]
    pub amt: Option<String>,
    #[serde(default)]
    pub dec: Option<String>,
}

pub struct Zrc20Engine {
    db: Db,
}

impl Zrc20Engine {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Process an inscription event
    /// event_type: "inscribe" or "transfer" (for when inscription is moved)
    pub fn process(
        &self,
        event_type: &str,
        inscription_id: &str,
        sender: &str,
        receiver: Option<&str>,
        content: &str,
    ) -> Result<()> {
        // Parse and validate JSON
        let op = match self.parse_and_validate(content) {
            Ok(op) => op,
            Err(e) => {
                tracing::debug!("ZRC-20 validation failed: {}", e);
                return Err(e); // Return error so it gets logged by indexer
            }
        };

        match (op.op.as_str(), event_type) {
            ("deploy", "inscribe") => self.handle_deploy_inscribe(&op, inscription_id, sender),
            ("mint", "inscribe") => self.handle_mint_inscribe(&op, inscription_id, sender),
            ("transfer", "inscribe") => self.handle_transfer_inscribe(&op, inscription_id, sender),
            ("transfer", "transfer") => self.handle_transfer_transfer(inscription_id, receiver),
            _ => Ok(()),
        }
    }

    /// Strict BRC-20 validation
    fn parse_and_validate(&self, content: &str) -> Result<Zrc20Operation> {
        // Must be valid JSON (not JSON5, no trailing commas)
        let op: Zrc20Operation = serde_json::from_str(content.trim())?;

        // Protocol must be "zrc-20" (case-sensitive for p field)
        if op.p != "zrc-20" {
            return Err(anyhow::anyhow!("Invalid protocol"));
        }

        // Op must be lowercase
        if op.op != op.op.to_lowercase() {
            return Err(anyhow::anyhow!("Op must be lowercase"));
        }

        // Normalize ticker to lowercase (BRC-20 is case-insensitive)
        let normalized_tick = op.tick.to_lowercase();

        // Validate ticker byte length (4-5 bytes UTF-8)
        let tick_bytes = normalized_tick.as_bytes().len();
        if tick_bytes < 4 || tick_bytes > 5 {
            return Err(anyhow::anyhow!("Ticker must be 4-5 bytes"));
        }

        // Update the operation with normalized ticker
        let mut op = op;
        op.tick = normalized_tick;

        // Validate numeric fields are strings and properly formatted
        if let Some(ref max) = op.max {
            self.validate_numeric_string(max, &op.dec)?;
        }
        if let Some(ref lim) = op.lim {
            self.validate_numeric_string(lim, &op.dec)?;
        }
        if let Some(ref amt) = op.amt {
            self.validate_numeric_string(amt, &op.dec)?;
        }
        if let Some(ref dec) = op.dec {
            self.validate_decimals(dec)?;
        }

        Ok(op)
    }

    fn validate_numeric_string(&self, value: &str, dec: &Option<String>) -> Result<()> {
        // Empty string is invalid
        if value.is_empty() {
            return Err(anyhow::anyhow!("Empty numeric string"));
        }

        // 0 is invalid (except for dec field, handled separately)
        if value == "0" {
            return Err(anyhow::anyhow!("Zero is invalid for this field"));
        }

        // Must contain only digits and at most one dot
        let dot_count = value.chars().filter(|&c| c == '.').count();
        if dot_count > 1 {
            return Err(anyhow::anyhow!("Multiple dots in numeric string"));
        }

        // Cannot start or end with dot
        if value.starts_with('.') || value.ends_with('.') {
            return Err(anyhow::anyhow!("Numeric string cannot start/end with dot"));
        }

        // All characters must be digits or dot
        if !value.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err(anyhow::anyhow!("Invalid characters in numeric string"));
        }

        // Check decimal precision
        if let Some(dot_pos) = value.find('.') {
            let decimal_places = value.len() - dot_pos - 1;
            let max_decimals = if let Some(d) = dec {
                d.parse::<usize>().unwrap_or(18)
            } else {
                18
            };

            if decimal_places > max_decimals {
                return Err(anyhow::anyhow!("Too many decimal places"));
            }
        }

        // Check max value (uint64_max)
        let _numeric_value: u64 = value
            .replace('.', "")
            .parse()
            .map_err(|_| anyhow::anyhow!("Value exceeds uint64_max"))?;

        Ok(())
    }

    fn validate_decimals(&self, dec: &str) -> Result<()> {
        // Dec can be 0
        if dec.is_empty() {
            return Err(anyhow::anyhow!("Empty decimals string"));
        }

        // Must be only digits
        if !dec.chars().all(|c| c.is_ascii_digit()) {
            return Err(anyhow::anyhow!("Decimals must be digits only"));
        }

        // Max 18
        let dec_value: u8 = dec
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid decimals value"))?;

        if dec_value > 18 {
            return Err(anyhow::anyhow!("Decimals cannot exceed 18"));
        }

        Ok(())
    }

    fn handle_deploy_inscribe(
        &self,
        op: &Zrc20Operation,
        inscription_id: &str,
        deployer: &str,
    ) -> Result<()> {
        let max = op.max.as_ref().ok_or(anyhow::anyhow!("Missing max"))?;
        let lim = op.lim.as_ref().unwrap_or(max); // If lim not set, equals max
        let dec = op.dec.as_ref().map(|s| s.as_str()).unwrap_or("18"); // Default 18

        let token_info = serde_json::json!({
            "tick": op.tick.to_lowercase(),
            "max": max,
            "lim": lim,
            "dec": dec,
            "deployer": deployer,
            "supply": "0",
            "inscription_id": inscription_id
        });

        self.db
            .deploy_token(&op.tick.to_lowercase(), &token_info.to_string())?;
        tracing::info!(
            "✅ Deployed token: {} (max: {}, lim: {}, dec: {})",
            op.tick,
            max,
            lim,
            dec
        );
        Ok(())
    }

    fn handle_mint_inscribe(
        &self,
        op: &Zrc20Operation,
        _inscription_id: &str,
        minter: &str,
    ) -> Result<()> {
        let amt_str = op.amt.as_ref().ok_or(anyhow::anyhow!("Missing amt"))?;

        // Load token info to check limits
        let token_info_str = self
            .db
            .get_token_info(&op.tick.to_lowercase())?
            .ok_or(anyhow::anyhow!("Token not found"))?;
        let token_info: serde_json::Value = serde_json::from_str(&token_info_str)?;

        let max: u64 = self.parse_amount(
            token_info["max"].as_str().unwrap_or("0"),
            token_info["dec"].as_str().unwrap_or("18"),
        )?;
        let lim: u64 = self.parse_amount(
            token_info["lim"].as_str().unwrap_or("0"),
            token_info["dec"].as_str().unwrap_or("18"),
        )?;
        let current_supply: u64 = self.parse_amount(
            token_info["supply"].as_str().unwrap_or("0"),
            token_info["dec"].as_str().unwrap_or("18"),
        )?;
        let amt: u64 = self.parse_amount(amt_str, token_info["dec"].as_str().unwrap_or("18"))?;

        // Validate mint amount
        if amt > lim {
            return Err(anyhow::anyhow!("Mint amount exceeds limit"));
        }

        if current_supply + amt > max {
            return Err(anyhow::anyhow!("Max supply exceeded"));
        }

        // Update supply
        self.db
            .update_token_supply(&op.tick.to_lowercase(), current_supply + amt)?;

        // Increase both available and overall balance for minter
        self.db
            .update_balance(minter, &op.tick.to_lowercase(), amt as i64, amt as i64)?;

        Ok(())
    }

    fn handle_transfer_inscribe(
        &self,
        op: &Zrc20Operation,
        inscription_id: &str,
        sender: &str,
    ) -> Result<()> {
        let amt_str = op.amt.as_ref().ok_or(anyhow::anyhow!("Missing amt"))?;

        // Get token decimals
        let token_info_str = self
            .db
            .get_token_info(&op.tick.to_lowercase())?
            .ok_or(anyhow::anyhow!("Token not found"))?;
        let token_info: serde_json::Value = serde_json::from_str(&token_info_str)?;
        let amt: u64 = self.parse_amount(amt_str, token_info["dec"].as_str().unwrap_or("18"))?;

        // Check available balance
        let balance = self.db.get_balance(sender, &op.tick.to_lowercase())?;
        if balance.available < amt {
            return Err(anyhow::anyhow!("Insufficient available balance"));
        }

        // Create transfer inscription (locks the amount)
        let transfer_data = serde_json::json!({
            "tick": op.tick.to_lowercase(),
            "amt": amt,
            "sender": sender
        });

        self.db
            .create_transfer_inscription(inscription_id, &transfer_data.to_string())?;

        // Decrease available balance (overall stays same)
        self.db
            .update_balance(sender, &op.tick.to_lowercase(), -(amt as i64), 0)?;

        Ok(())
    }

    fn handle_transfer_transfer(&self, inscription_id: &str, receiver: Option<&str>) -> Result<()> {
        // Check if inscription is already used
        if self.db.is_inscription_used(inscription_id)? {
            return Err(anyhow::anyhow!("Transfer inscription already used"));
        }

        // Get transfer inscription data
        let transfer_data_str = self
            .db
            .get_transfer_inscription(inscription_id)?
            .ok_or(anyhow::anyhow!("Transfer inscription not found"))?;
        let transfer_data: serde_json::Value = serde_json::from_str(&transfer_data_str)?;

        let tick = transfer_data["tick"]
            .as_str()
            .ok_or(anyhow::anyhow!("Invalid tick"))?;
        let amt = transfer_data["amt"]
            .as_u64()
            .ok_or(anyhow::anyhow!("Invalid amount"))?;
        let sender = transfer_data["sender"]
            .as_str()
            .ok_or(anyhow::anyhow!("Invalid sender"))?;

        let receiver = receiver.unwrap_or(sender); // If sent to self, increase available balance

        if receiver == sender {
            // Self-transfer: increase available balance
            self.db.update_balance(sender, tick, amt as i64, 0)?;
        } else {
            // Transfer to another address
            // Decrease sender's overall balance
            self.db.update_balance(sender, tick, 0, -(amt as i64))?;
            // Increase receiver's both balances
            self.db
                .update_balance(receiver, tick, amt as i64, amt as i64)?;
        }

        // Mark inscription as used
        self.db.mark_inscription_used(inscription_id)?;

        Ok(())
    }

    /// Parse amount string with decimals support using overflow-safe arithmetic.
    fn parse_amount(&self, amount_str: &str, decimals: &str) -> Result<u64> {
        let dec: u32 = decimals.parse().unwrap_or(18);
        let scale = 10u128.pow(dec);

        let (whole_part, frac_part) = match amount_str.split_once('.') {
            Some((whole, frac)) => (whole, frac),
            None => (amount_str, ""),
        };

        let whole: u128 = if whole_part.is_empty() {
            0
        } else {
            whole_part.parse::<u128>()?
        };

        let mut frac_string = frac_part.to_string();
        if frac_string.len() > dec as usize {
            return Err(anyhow::anyhow!("Too many decimal places"));
        }
        while frac_string.len() < dec as usize {
            frac_string.push('0');
        }

        let frac_value: u128 = if frac_string.is_empty() {
            0
        } else {
            frac_string.parse::<u128>()?
        };

        let whole_scaled = whole
            .checked_mul(scale)
            .ok_or_else(|| anyhow::anyhow!("Amount exceeds maximum representable value"))?;
        let total = whole_scaled
            .checked_add(frac_value)
            .ok_or_else(|| anyhow::anyhow!("Amount exceeds maximum representable value"))?;

        if total > u64::MAX as u128 {
            return Err(anyhow::anyhow!("Amount exceeds u64 storage"));
        }

        Ok(total as u64)
    }
}
