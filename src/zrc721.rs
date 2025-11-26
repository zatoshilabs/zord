use crate::db::Db;
use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Zrc721Operation {
    p: String,
    op: String,
    #[serde(default)]
    tick: Option<String>,
    #[serde(default)]
    collection: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    max: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    meta: Option<serde_json::Value>,
}

pub struct Zrc721Engine {
    db: Db,
}

impl Zrc721Engine {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn process(
        &self,
        event_type: &str,
        inscription_id: &str,
        sender: &str,
        content: &str,
    ) -> Result<()> {
        if event_type != "inscribe" {
            return Ok(());
        }

        let op: Zrc721Operation = serde_json::from_str(content.trim())?;
        if op.p.to_lowercase() != "zrc-721" {
            return Err(anyhow::anyhow!("Not a ZRC-721 payload"));
        }

        match op.op.as_str() {
            "deploy" => self.handle_deploy(&op, inscription_id, sender),
            "mint" => self.handle_mint(&op, inscription_id, sender),
            _ => Err(anyhow::anyhow!("Unsupported op")),
        }
    }

    fn handle_deploy(
        &self,
        op: &Zrc721Operation,
        inscription_id: &str,
        deployer: &str,
    ) -> Result<()> {
        let tick = op
            .tick
            .as_ref()
            .or(op.collection.as_ref())
            .ok_or(anyhow::anyhow!("Missing collection/tick"))?
            .to_lowercase();
        let name = op.name.as_ref().ok_or(anyhow::anyhow!("Missing name"))?;
        let symbol = op.symbol.as_ref().unwrap_or(name);
        let max = op.max.as_ref().ok_or(anyhow::anyhow!("Missing max"))?;

        let payload = serde_json::json!({
            "tick": tick,
            "name": name,
            "symbol": symbol,
            "max": max,
            "minted": 0,
            "deployer": deployer,
            "inscription_id": inscription_id
        });

        self.db.register_zrc721_collection(&tick, &payload)
    }

    fn handle_mint(
        &self,
        op: &Zrc721Operation,
        inscription_id: &str,
        sender: &str,
    ) -> Result<()> {
        let tick = op
            .tick
            .as_ref()
            .or(op.collection.as_ref())
            .ok_or(anyhow::anyhow!("Missing collection/tick"))?
            .to_lowercase();
        let token_id = op
            .id
            .as_ref()
            .ok_or(anyhow::anyhow!("Missing token id"))?;

        // Validate that the token id is numeric (common convention for 0..max indexing)
        if token_id.chars().any(|c| !c.is_ascii_digit()) {
            return Err(anyhow::anyhow!("Token id must be numeric"));
        }
        let owner = op.to.as_deref().unwrap_or(sender);

        let metadata = op.meta.clone().unwrap_or_else(|| serde_json::json!({}));
        self.db.insert_zrc721_token(&tick, token_id, owner, inscription_id, &metadata)
    }
}
