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
        // Only process text/plain inscriptions
        if content_type != "text/plain" {
            return Ok(()); // Ignore non-text inscriptions
        }

        let name = content.trim();

        // Validate and register
        if self.validate_name(name).is_ok() {
            self.handle_registration(name, inscription_id, owner)?;
        }

        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<()> {
        // Must end with .zec or .zcash
        if !name.ends_with(".zec") && !name.ends_with(".zcash") {
            return Err(anyhow::anyhow!("Name must end with .zec or .zcash"));
        }

        // Extract base name (without extension)
        let base_name = if name.ends_with(".zcash") {
            &name[..name.len() - 6]
        } else {
            &name[..name.len() - 4]
        };

        // Base name must not be empty
        if base_name.is_empty() {
            return Err(anyhow::anyhow!("Name cannot be empty"));
        }

        // Total length check (reasonable limit)
        if name.len() > 253 {
            return Err(anyhow::anyhow!("Name too long (max 253 characters)"));
        }

        Ok(())
    }

    fn handle_registration(&self, name: &str, inscription_id: &str, owner: &str) -> Result<()> {
        // Names are case-insensitive but preserve original case in display
        let name_lower = name.to_lowercase();

        // Check if name already exists (first-is-first)
        if self.db.get_name(&name_lower)?.is_some() {
            return Err(anyhow::anyhow!("Name already registered"));
        }

        // Register the name
        let name_data = serde_json::json!({
            "name": name, // Preserve original case for display
            "name_lower": name_lower,
            "owner": owner,
            "inscription_id": inscription_id,
        });

        self.db.register_name(&name_lower, &name_data.to_string())?;

        tracing::info!("Registered name: {} -> {}", name, owner);

        Ok(())
    }
}
