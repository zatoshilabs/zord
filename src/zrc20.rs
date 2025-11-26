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
        txid: Option<&str>,
        assigned_vout: Option<u32>,
    ) -> Result<()> {
        // Parse and validate JSON
        let op = match self.parse_and_validate(content) {
            Ok(op) => op,
            Err(e) => {
                tracing::debug!("ZRC-20 validation failed: {}", e);
                return Err(e); // bubble up so the caller logs the failure
            }
        };

        match (op.op.as_str(), event_type) {
            ("deploy", "inscribe") => self.handle_deploy_inscribe(&op, inscription_id, sender),
            ("mint", "inscribe") => self.handle_mint_inscribe(&op, inscription_id, sender),
            ("transfer", "inscribe") => self.handle_transfer_inscribe(&op, inscription_id, sender, txid, assigned_vout),
            ("transfer", "transfer") => self.handle_transfer_transfer(inscription_id, receiver),
            _ => Ok(()),
        }
    }

    /// Strict BRC-20 validation
    fn parse_and_validate(&self, content: &str) -> Result<Zrc20Operation> {
        // Payloads must be strict JSON
        let op: Zrc20Operation = serde_json::from_str(content.trim())?;

        // Protocol marker must normalize to zrc-20
        if op.p.to_lowercase() != "zrc-20" {
            return Err(anyhow::anyhow!("Invalid protocol"));
        }

        // Canonical op codes are lowercase
        if op.op != op.op.to_lowercase() {
            return Err(anyhow::anyhow!("Op must be lowercase"));
        }

        // Tick comparison uses lowercase to avoid duplicates
        let normalized_tick = op.tick.to_lowercase();

        // Enforce BRC/ZRC ticker length limits
        let tick_bytes = normalized_tick.as_bytes().len();
        if tick_bytes < 4 || tick_bytes > 5 {
            return Err(anyhow::anyhow!("Ticker must be 4-5 bytes"));
        }

        // Persist the normalized ticker back into the struct
        let mut op = op;
        op.tick = normalized_tick;

        // Numeric fields must be strings with optional fractional parts
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
        // Reject empty strings
        if value.is_empty() {
            return Err(anyhow::anyhow!("Empty numeric string"));
        }

        // Treat literal 0 as invalid for value fields (decimals handled separately)
        if value == "0" {
            return Err(anyhow::anyhow!("Zero is invalid for this field"));
        }

        // Allow digits plus a single decimal point
        let dot_count = value.chars().filter(|&c| c == '.').count();
        if dot_count > 1 {
            return Err(anyhow::anyhow!("Multiple dots in numeric string"));
        }

        // Strip obvious malformed inputs
        if value.starts_with('.') || value.ends_with('.') {
            return Err(anyhow::anyhow!("Numeric string cannot start/end with dot"));
        }

        // ASCII-only numbers are accepted
        if !value.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err(anyhow::anyhow!("Invalid characters in numeric string"));
        }

        // Enforce declared decimal precision if a fractional part is present
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

        // Guard against values exceeding u64 once scaled
        let _numeric_value: u64 = value
            .replace('.', "")
            .parse()
            .map_err(|_| anyhow::anyhow!("Value exceeds uint64_max"))?;

        Ok(())
    }

    fn validate_decimals(&self, dec: &str) -> Result<()> {
        // Decimals may be zero
        if dec.is_empty() {
            return Err(anyhow::anyhow!("Empty decimals string"));
        }

        // Decimal field must be numeric
        if !dec.chars().all(|c| c.is_ascii_digit()) {
            return Err(anyhow::anyhow!("Decimals must be digits only"));
        }

        // BRC/ZRC cap decimals at 18
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
        let lim = op.lim.as_ref().unwrap_or(max); // default lim=max
        let dec = op.dec.as_ref().map(|s| s.as_str()).unwrap_or("18"); // default decimals

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

        // Pull token metadata so we can enforce deployment limits
        let token_info_str = self
            .db
            .get_token_info(&op.tick.to_lowercase())?
            .ok_or(anyhow::anyhow!("Token not found"))?;
        let token_info: serde_json::Value = serde_json::from_str(&token_info_str)?;

        let max: u128 = self.parse_amount(
            token_info["max"].as_str().unwrap_or("0"),
            token_info["dec"].as_str().unwrap_or("18"),
        )?;
        let lim: u128 = self.parse_amount(
            token_info["lim"].as_str().unwrap_or("0"),
            token_info["dec"].as_str().unwrap_or("18"),
        )?;
        let current_supply: u128 = token_info["supply"].as_str()
            .and_then(|s| s.parse::<u128>().ok())
            .unwrap_or(0);
        let amt: u128 = self.parse_amount(amt_str, token_info["dec"].as_str().unwrap_or("18"))?;

        // Ensure mint fits within per-address limit and total supply
        if amt > lim {
            return Err(anyhow::anyhow!("Mint amount exceeds limit"));
        }

        if current_supply + amt > max {
            return Err(anyhow::anyhow!("Max supply exceeded"));
        }

        // Atomically bump supply and credit holder balance to avoid drift
        self.db.mint_credit_atomic(&op.tick.to_lowercase(), minter, amt)?;

        Ok(())
    }

    fn handle_transfer_inscribe(
        &self,
        op: &Zrc20Operation,
        inscription_id: &str,
        sender: &str,
        txid: Option<&str>,
        assigned_vout: Option<u32>,
    ) -> Result<()> {
        let amt_str = op.amt.as_ref().ok_or(anyhow::anyhow!("Missing amt"))?;

        // Normalize the requested transfer amount using token decimals
        let token_info_str = self
            .db
            .get_token_info(&op.tick.to_lowercase())?
            .ok_or(anyhow::anyhow!("Token not found"))?;
        let token_info: serde_json::Value = serde_json::from_str(&token_info_str)?;
        let amt: u128 = self.parse_amount(amt_str, token_info["dec"].as_str().unwrap_or("18"))?;

        // Require unlocked balance before staging the transfer
        let balance = self.db.get_balance(sender, &op.tick.to_lowercase())?;
        if balance.available < amt {
            return Err(anyhow::anyhow!("Insufficient available balance"));
        }

        // Record the intent so the reveal can settle it later
        let transfer_data = serde_json::json!({
            "tick": op.tick.to_lowercase(),
            "amt": amt.to_string(),
            "sender": sender
        });

        self.db
            .create_transfer_inscription(inscription_id, &transfer_data.to_string())?;

        // Register the actual outpoint for reveal detection when available
        if let (Some(txid), Some(vout)) = (txid, assigned_vout) {
            let _ = self.db.register_transfer_outpoint(txid, vout, inscription_id);
        }

        // Lock the amount by reducing only the spendable balance
        self.db
            .update_balance(sender, &op.tick.to_lowercase(), -(amt as i128), 0)?;

        Ok(())
    }

    fn handle_transfer_transfer(&self, inscription_id: &str, receiver: Option<&str>) -> Result<()> {
        // Prevent double-settlement of a transfer inscription
        if self.db.is_inscription_used(inscription_id)? {
            return Err(anyhow::anyhow!("Transfer inscription already used"));
        }

        // Load the staged transfer data
        let transfer_data_str = self
            .db
            .get_transfer_inscription(inscription_id)?
            .ok_or(anyhow::anyhow!("Transfer inscription not found"))?;
        let transfer_data: serde_json::Value = serde_json::from_str(&transfer_data_str)?;

        let tick = transfer_data["tick"]
            .as_str()
            .ok_or(anyhow::anyhow!("Invalid tick"))?;
        let amt = transfer_data["amt"]
            .as_str()
            .ok_or(anyhow::anyhow!("Invalid amount"))?
            .parse::<u128>()?;
        let sender = transfer_data["sender"]
            .as_str()
            .ok_or(anyhow::anyhow!("Invalid sender"))?;

        let receiver = receiver.unwrap_or(sender); // default to self when reveal spends locally

        if receiver == sender {
            // Unlock the funds if they ultimately returned to sender
            self.db.update_balance(sender, tick, amt as i128, 0)?;
        } else {
            // Move value to the receiver and debit the sender
            self.db.update_balance(sender, tick, 0, -(amt as i128))?;
            self.db
                .update_balance(receiver, tick, amt as i128, amt as i128)?;
        }

        // Flag the inscription so reveal cannot replay
        self.db.mark_inscription_used(inscription_id)?;

        Ok(())
    }

    /// Public entry to settle a staged transfer when the inscription is revealed (spent).
    pub fn settle_transfer(&self, inscription_id: &str, receiver: Option<&str>) -> Result<()> {
        self.handle_transfer_transfer(inscription_id, receiver)
    }

    /// Parse amount string with decimals support using overflow-safe arithmetic.
    fn parse_amount(&self, amount_str: &str, decimals: &str) -> Result<u128> {
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

        Ok(total)
    }
}
