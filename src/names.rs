use crate::db::Db;
use anyhow::Result;

pub struct NamesEngine {
    db: Db,
}

impl NamesEngine {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Process a plain text name inscription
    /// Content should be just the name itself: "satoshi.zec" or "🔥fire.zcash"
    pub fn process(
        &self,
        inscription_id: &str,
        owner: &str,
        content: &str,
        content_type: &str,
    ) -> Result<()> {
        // Ignore anything other than plain text payloads
        if content_type != "text/plain" {
            return Ok(());
        }

        let name = content.trim();

        // Accept first writer only
        if self.validate_name(name).is_ok() {
            self.handle_registration(name, inscription_id, owner)?;
        }

        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<()> {
        // Only .zec and .zcash suffixes are supported
        if !name.ends_with(".zec") && !name.ends_with(".zcash") {
            return Err(anyhow::anyhow!("Name must end with .zec or .zcash"));
        }

        // Must be a single token: reject any internal whitespace (spaces, tabs, newlines, etc.)
        if name.chars().any(|c| c.is_whitespace()) {
            return Err(anyhow::anyhow!(
                "Name content must be a single token without spaces (e.g., alice.zec)"
            ));
        }

        // Strip the extension for validation
        let base_name = if name.ends_with(".zcash") {
            &name[..name.len() - 6]
        } else {
            &name[..name.len() - 4]
        };

        // Disallow empty labels (e.g. ".zec")
        if base_name.is_empty() {
            return Err(anyhow::anyhow!("Name cannot be empty"));
        }

        // Simple length guard
        if name.len() > 253 {
            return Err(anyhow::anyhow!("Name too long (max 253 characters)"));
        }

        Ok(())
    }

    fn handle_registration(&self, name: &str, inscription_id: &str, owner: &str) -> Result<()> {
        // Store lower-case key, but keep caller formatting for display
        let name_lower = name.to_lowercase();

        // First registration wins
        if self.db.get_name(&name_lower)?.is_some() {
            return Err(anyhow::anyhow!("Name already registered"));
        }

        let name_data = serde_json::json!({
            "name": name,
            "name_lower": name_lower,
            "owner": owner,
            "inscription_id": inscription_id,
        });

        self.db.register_name(&name_lower, &name_data.to_string())?;

        tracing::info!("Registered name: {} -> {}", name, owner);

        Ok(())
    }
}
