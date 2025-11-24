use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;

#[derive(Clone)]
pub struct ZcashRpcClient {
    url: String,
    client: reqwest::Client,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct BlockResponse {
    pub height: u64,
    pub hash: String,
    pub tx: Vec<String>, // Tx IDs
    pub time: u64,
    pub previousblockhash: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct TxResponse {
    pub txid: String,
    pub hex: String,
    pub vin: Vec<Vin>,
    pub vout: Vec<Vout>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Vin {
    pub txid: Option<String>,
    pub vout: Option<u32>,
    #[serde(rename = "scriptSig")]
    pub script_sig: Option<ScriptSig>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ScriptSig {
    pub hex: String,
    pub asm: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Vout {
    pub value: f64,
    pub n: u32,
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: ScriptPubKey,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ScriptPubKey {
    pub hex: String,
    pub asm: String,
    pub r#type: String,
    pub addresses: Option<Vec<String>>,
}

impl ZcashRpcClient {
    pub fn new() -> Self {
        let url = env::var("ZCASH_RPC_URL")
            .unwrap_or_else(|_| "https://rpc.zatoshi.market/api/rpc".to_string());

        let username = env::var("ZCASH_RPC_USERNAME").unwrap_or_else(|_| "zatoshi".to_string());
        let password = env::var("ZCASH_RPC_PASSWORD")
            .expect("ZCASH_RPC_PASSWORD must be provided via environment variable");

        // Create Basic Auth header
        let auth = format!("{}:{}", username, password);
        let auth_header = format!(
            "Basic {}",
            general_purpose::STANDARD.encode(auth.as_bytes())
        );

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&auth_header).expect("Invalid auth header"),
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build client");

        tracing::info!("Initialized Zcash RPC client: {}", url);

        Self { url, client }
    }

    async fn call<T: Serialize>(&self, method: &str, params: T) -> Result<Value> {
        let body = serde_json::json!({
            "jsonrpc": "1.0",
            "id": "zord",
            "method": method,
            "params": params
        });

        let res = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await?
            .json::<Value>()
            .await?;

        if let Some(err) = res.get("error") {
            if !err.is_null() {
                return Err(anyhow::anyhow!("RPC Error: {:?}", err));
            }
        }

        Ok(res["result"].clone())
    }

    pub async fn get_block_count(&self) -> Result<u64> {
        let res = self.call("getblockcount", Vec::<Value>::new()).await?;
        Ok(res.as_u64().unwrap_or(0))
    }

    pub async fn get_block_hash(&self, height: u64) -> Result<String> {
        let res = self
            .call("getblockhash", vec![serde_json::json!(height)])
            .await?;
        Ok(res.as_str().unwrap_or("").to_string())
    }

    pub async fn get_block(&self, hash: &str) -> Result<BlockResponse> {
        let res = self
            .call(
                "getblock",
                vec![serde_json::json!(hash), serde_json::json!(1)],
            )
            .await?;
        serde_json::from_value(res).map_err(|e| anyhow::anyhow!("Failed to parse block: {}", e))
    }

    pub async fn get_raw_transaction(&self, txid: &str) -> Result<TxResponse> {
        let res = self
            .call(
                "getrawtransaction",
                vec![serde_json::json!(txid), serde_json::json!(1)],
            )
            .await?;
        serde_json::from_value(res).map_err(|e| anyhow::anyhow!("Failed to parse tx: {}", e))
    }
}
