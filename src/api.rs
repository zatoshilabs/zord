use crate::db::Db;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

const FRONT_HTML: &str = include_str!("../web/index.html");
const MAX_PAGE_SIZE: usize = 200;

#[derive(Deserialize)]
struct PaginationParams {
    page: Option<usize>,
    limit: Option<usize>,
}

impl PaginationParams {
    fn resolve(&self) -> (usize, usize) {
        let page = self.page.unwrap_or(0);
        let limit = self.limit.unwrap_or(24).clamp(1, MAX_PAGE_SIZE);
        (page, limit)
    }
}

#[derive(Clone)]
pub struct AppState {
    db: Db,
}

#[derive(Serialize)]
struct PaginatedResponse<T> {
    page: usize,
    limit: usize,
    total: u64,
    has_more: bool,
    items: Vec<T>,
}

#[derive(Serialize)]
struct InscriptionSummary {
    id: String,
    content_type: String,
    sender: String,
    txid: String,
    block_height: Option<u64>,
    content_length: usize,
    preview_text: Option<String>,
}

#[derive(Serialize)]
struct TokenSummary {
    ticker: String,
    max: String,
    max_base_units: String,
    supply: String,
    supply_base_units: String,
    lim: String,
    dec: String,
    deployer: String,
    inscription_id: String,
    progress: f64,
}

#[derive(Serialize)]
struct NameSummary {
    name: String,
    owner: String,
    inscription_id: String,
}

pub async fn start_api(db: Db, port: u16) {
    let state = AppState { db };

    let app = Router::new()
        // Frontend entry + component redirects
        .route("/", get(frontpage))
        .route("/tokens", get(tokens_redirect))
        .route("/names", get(names_redirect))
        .route("/api", get(api_docs))
        // JSON feeds consumed by the new UI
        .route("/api/v1/inscriptions", get(get_inscriptions_feed))
        .route("/api/v1/tokens", get(get_tokens_feed))
        .route("/api/v1/names", get(get_names_feed))
        .route("/api/v1/status", get(get_status))
        // Ordinals-compatible routes
        .route("/inscription/:id", get(get_inscription))
        .route("/inscriptions", get(get_recent_inscriptions))
        .route("/content/:id", get(get_inscription_content))
        .route("/preview/:id", get(get_inscription_preview))
        .route("/block/:query", get(get_block))
        .route("/tx/:txid", get(get_transaction))
        .route("/status", get(get_status))
        // Backwards-compatibility + helpers
        .route("/health", get(health))
        .route("/block/height", get(get_block_height))
        .route(
            "/inscription/number/:number",
            get(get_inscription_by_number),
        )
        .route(
            "/address/:address/inscriptions",
            get(get_address_inscriptions),
        )
        .route("/token/:tick", get(get_token_info))
        .route("/token/:tick/balance/:address", get(get_balance))
        .route("/tokens/list", get(get_all_tokens_api))
        .route("/names/list", get(get_all_names_api))
        .route("/name/:name", get(get_name_info))
        .route("/resolve/:name", get(resolve_name))
        // Static files (must be last)
        .nest_service("/static", ServeDir::new("web"))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("API listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn get_block_height(State(state): State<AppState>) -> Json<serde_json::Value> {
    let height = state.db.get_latest_indexed_height().unwrap_or(None);
    Json(serde_json::json!({ "height": height }))
}

async fn get_recent_inscriptions(State(state): State<AppState>) -> Json<serde_json::Value> {
    let inscriptions = state.db.get_inscriptions_page(0, 50).unwrap_or_default();
    let data: Vec<serde_json::Value> = inscriptions.into_iter().map(|(id, meta)| {
        serde_json::json!({
            "id": id,
            "meta": serde_json::from_str::<serde_json::Value>(&meta).unwrap_or(serde_json::Value::String(meta))
        })
    }).collect();
    Json(serde_json::json!(data))
}

async fn get_inscription(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let meta = match state.db.get_inscription(&id).unwrap_or(None) {
        Some(m) => m,
        None => return Html(r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Inscription Not Found</title>
    <style>
        body { font-family: monospace; background: #111; color: #fff; padding: 40px; text-align: center; }
        a { color: #fff; }
    </style>
</head>
<body>
    <h1>Inscription Not Found</h1>
    <a href="/">← Back to inscriptions</a>
</body>
</html>"#).into_response(),
    };

    let val: serde_json::Value = match serde_json::from_str(&meta) {
        Ok(v) => v,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid metadata").into_response(),
    };

    let content_type_raw = val["content_type"].as_str().unwrap_or("text/plain");
    let content = val["content"].as_str().unwrap_or("");
    let content_hex = val["content_hex"].as_str().unwrap_or("");
    let sender_raw = val["sender"].as_str().unwrap_or("unknown");
    let txid_raw = val["txid"].as_str().unwrap_or("");

    let sender = html_escape::encode_text(sender_raw).to_string();
    let txid = html_escape::encode_text(txid_raw).to_string();
    let content_type = html_escape::encode_text(content_type_raw).to_string();
    let id_text = html_escape::encode_text(&id).to_string();
    let id_attr = html_escape::encode_double_quoted_attribute(&id).to_string();
    let short_id: String = id_text.chars().take(16).collect();

    // Render content preview based on type (following Ordinals rendering standards)
    let content_preview = if content_type_raw.starts_with("image/") {
        // Determine image-rendering based on format
        let rendering = if content_type_raw == "image/avif" || content_type_raw == "image/jxl" {
            "auto"
        } else {
            // For pixel art and most images, use pixelated for upscaling
            "pixelated"
        };

        format!(
            r#"<div class="content-preview">
            <div class="image-container">
                <img src="/content/{}" class="inscription-image" style="image-rendering: {};">
            </div>
            <div style="margin-top: 10px; color: #999; font-size: 0.9em;">
                <a href="/content/{}" target="_blank" style="color: #999;">view full size</a>
            </div>
        </div>"#,
            id_attr, rendering, id_attr
        )
    } else if content_type_raw == "application/json" {
        let formatted = serde_json::to_string_pretty(
            &serde_json::from_str::<serde_json::Value>(content).unwrap_or_default(),
        )
        .unwrap_or_else(|_| content.to_string());
        format!(
            r#"<div class="content-preview">
            <pre>{}</pre>
        </div>"#,
            html_escape::encode_text(&formatted)
        )
    } else if content_type_raw.starts_with("text/") {
        format!(
            r#"<div class="content-preview">
            <pre>{}</pre>
        </div>"#,
            html_escape::encode_text(content)
        )
    } else {
        let size = content_hex.len() / 2;
        format!(
            r#"<div class="content-preview">
            <p style="color: #999;">Binary content ({} bytes)</p>
            <a href="/content/{}" class="button">Download</a>
        </div>"#,
            size, id_attr
        )
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Inscription {}</title>
    <style>
        * {{ margin: 0; padding: 0; box-sizing: border-box; }}
        body {{
            font-family: monospace;
            background: #111;
            color: #fff;
            padding: 40px 20px;
            line-height: 1.6;
        }}
        .container {{ max-width: 1000px; margin: 0 auto; }}
        .back-link {{
            color: #999;
            text-decoration: none;
            display: inline-block;
            margin-bottom: 30px;
            transition: color 0.2s;
        }}
        .back-link:hover {{ color: #fff; }}
        h1 {{
            font-size: 1.5em;
            margin-bottom: 30px;
            word-break: break-all;
        }}
        .meta {{
            background: #1a1a1a;
            border: 1px solid #333;
            border-radius: 8px;
            padding: 20px;
            margin-top: 30px;
        }}
        .meta-row {{
            display: flex;
            padding: 10px 0;
            border-bottom: 1px solid #333;
        }}
        .meta-row:last-child {{ border-bottom: none; }}
        .meta-label {{
            color: #999;
            min-width: 120px;
        }}
        .meta-value {{
            color: #fff;
            word-break: break-all;
        }}
        .content-preview {{
            background: #1a1a1a;
            border: 1px solid #333;
            border-radius: 8px;
            padding: 20px;
        }}
        .image-container {{
            width: 576px;
            height: 576px;
            max-width: 100%;
            display: flex;
            align-items: center;
            justify-content: center;
            background: #0a0a0a;
            border-radius: 4px;
            overflow: hidden;
        }}
        .inscription-image {{
            max-width: 100%;
            max-height: 100%;
            width: auto;
            height: auto;
            display: block;
        }}
        pre {{
            white-space: pre-wrap;
            word-wrap: break-word;
            background: #0a0a0a;
            padding: 15px;
            border-radius: 4px;
            overflow-x: auto;
        }}
        .button {{
            display: inline-block;
            padding: 10px 20px;
            background: #333;
            color: #fff;
            text-decoration: none;
            border-radius: 4px;
            margin-top: 10px;
            transition: background 0.2s;
        }}
        .button:hover {{ background: #444; }}
    </style>
</head>
<body>
    <div class="container">
        <a href="/" class="back-link">← back</a>

        <h1>Inscription {}</h1>

        {}

        <div class="meta">
            <div class="meta-row">
                <div class="meta-label">id</div>
                <div class="meta-value">{}</div>
            </div>
            <div class="meta-row">
                <div class="meta-label">content type</div>
                <div class="meta-value">{}</div>
            </div>
            <div class="meta-row">
                <div class="meta-label">owner</div>
                <div class="meta-value">{}</div>
            </div>
            <div class="meta-row">
                <div class="meta-label">genesis transaction</div>
                <div class="meta-value">{}</div>
            </div>
        </div>
    </div>
</body>
</html>"#,
        short_id, id_text, content_preview, id_text, content_type, sender, txid
    );

    Html(html).into_response()
}

async fn get_inscription_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let meta = match state.db.get_inscription(&id).unwrap_or(None) {
        Some(m) => m,
        None => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };

    let val: serde_json::Value = match serde_json::from_str(&meta) {
        Ok(v) => v,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid metadata").into_response(),
    };

    let content_type = val["content_type"].as_str().unwrap_or("text/plain");
    let content_hex = val["content_hex"].as_str().unwrap_or("");

    // Decode hex to bytes
    let content_bytes = match hex::decode(content_hex) {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid content data").into_response()
        }
    };

    // Return with proper content-type header
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type)],
        content_bytes,
    )
        .into_response()
}

async fn get_inscription_by_number(
    State(state): State<AppState>,
    Path(number): Path<u64>,
) -> Json<serde_json::Value> {
    // We need to implement get_inscription_by_number in db.rs first?
    // Wait, I can access the table directly if I expose it, but better to add a method to Db.
    // Checking db.rs... I added the tables but not the accessor methods in the previous turn?
    // Ah, I see I added `INSCRIPTION_NUMBERS` table but didn't add a `get_inscription_by_number` method to `Db` struct in `src/db.rs`.
    // I should probably add the method to `src/db.rs` first.
    // But for now, let's assume I'll add it.

    // Actually, I'll implement the handler logic here assuming the DB method exists,
    // and then I will go update db.rs to ensure the method exists.
    // Wait, if I update api.rs first, it might fail to compile if I run check.
    // I should update db.rs FIRST.

    // Re-reading my thought process: I will update db.rs first in the next tool call.
    // But I am already in the replace_file_content for api.rs.
    // I will comment out the implementation or just put a placeholder,
    // OR I can just do the db.rs update in the next step and it's fine since I'm not compiling yet.

    // Let's write the handler code assuming `state.db.get_inscription_by_number(number)` exists.

    let id = state.db.get_inscription_by_number(number).unwrap_or(None);
    if let Some(inscription_id) = id {
        // Redirect or just return the inscription? Let's return the inscription.
        let meta = state.db.get_inscription(&inscription_id).unwrap_or(None);
        if let Some(m) = meta {
            let mut val = serde_json::from_str::<serde_json::Value>(&m)
                .unwrap_or(serde_json::Value::String(m));
            if let Some(obj) = val.as_object_mut() {
                obj.insert("id".to_string(), serde_json::Value::String(inscription_id));
                obj.insert("number".to_string(), serde_json::json!(number));
            }
            Json(val)
        } else {
            Json(serde_json::json!({ "error": "Inscription data missing" }))
        }
    } else {
        Json(serde_json::json!({ "error": "Not found" }))
    }
}

async fn get_address_inscriptions(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Json<serde_json::Value> {
    let inscriptions = state
        .db
        .get_inscriptions_by_address(&address)
        .unwrap_or_default();
    Json(serde_json::json!(inscriptions))
}

async fn get_token_info(
    State(state): State<AppState>,
    Path(tick): Path<String>,
) -> Json<serde_json::Value> {
    let info = state.db.get_token_info(&tick).unwrap_or(None);
    if let Some(i) = info {
        let val =
            serde_json::from_str::<serde_json::Value>(&i).unwrap_or(serde_json::Value::String(i));
        Json(val)
    } else {
        Json(serde_json::json!({ "error": "Not found" }))
    }
}

async fn get_balance(
    State(state): State<AppState>,
    Path((tick, address)): Path<(String, String)>,
) -> Json<serde_json::Value> {
    let balance = state
        .db
        .get_balance(&address, &tick)
        .unwrap_or(crate::db::Balance {
            available: 0,
            overall: 0,
        });
    Json(serde_json::json!({
        "tick": tick,
        "address": address,
        "available": balance.available,
        "overall": balance.overall
    }))
}

// Ordinals-compatible routes

async fn frontpage() -> Html<&'static str> {
    Html(FRONT_HTML)
}

async fn tokens_redirect() -> Redirect {
    Redirect::permanent("/#tokens")
}

async fn names_redirect() -> Redirect {
    Redirect::permanent("/#names")
}

async fn get_inscriptions_feed(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<InscriptionSummary>>, StatusCode> {
    let (page, limit) = params.resolve();
    let total = state.db.get_inscription_count().map_err(|err| {
        tracing::error!("inscription count error: {}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let rows = state.db.get_inscriptions_page(page, limit).map_err(|err| {
        tracing::error!("inscriptions page error: {}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let offset = (page as u64).saturating_mul(limit as u64);
    let has_more = offset + (rows.len() as u64) < total;

    let mut items = Vec::with_capacity(rows.len());
    for (id, payload) in rows {
        let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap_or_default();
        let content_type = parsed["content_type"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        let sender = parsed["sender"].as_str().unwrap_or("unknown").to_string();
        let txid = parsed["txid"].as_str().unwrap_or("").to_string();
        let block_height = parsed["block_height"].as_u64();
        let content_length = parsed["content_hex"]
            .as_str()
            .map(|hex| hex.len() / 2)
            .unwrap_or(0);
        let preview_text = build_preview(&content_type, &parsed);

        items.push(InscriptionSummary {
            id,
            content_type,
            sender,
            txid,
            block_height,
            content_length,
            preview_text,
        });
    }

    Ok(Json(PaginatedResponse {
        page,
        limit,
        total,
        has_more,
        items,
    }))
}

async fn get_tokens_feed(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<TokenSummary>>, StatusCode> {
    let (page, limit) = params.resolve();
    let total = state.db.get_token_count().map_err(|err| {
        tracing::error!("token count error: {}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let rows = state.db.get_tokens_page(page, limit).map_err(|err| {
        tracing::error!("token page error: {}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let offset = (page as u64).saturating_mul(limit as u64);
    let has_more = offset + (rows.len() as u64) < total;

    let mut items = Vec::with_capacity(rows.len());
    for (ticker, payload) in rows {
        if let Ok(info) = serde_json::from_str::<serde_json::Value>(&payload) {
            let max = info["max"].as_str().unwrap_or("0").to_string();
            let lim = info["lim"].as_str().unwrap_or(&max).to_string();
            let dec = info["dec"].as_str().unwrap_or("18").to_string();
            let dec_value = dec.parse::<u32>().unwrap_or(18);
            let deployer = info["deployer"].as_str().unwrap_or("unknown").to_string();
            let inscription_id = info["inscription_id"].as_str().unwrap_or("").to_string();
            let supply_base_units = info["supply"].as_str().unwrap_or("0").to_string();
            let supply_float = supply_base_units.parse::<f64>().unwrap_or(0.0);
            let display_supply = format_decimal(supply_float / 10u64.pow(dec_value) as f64);
            let max_base_units = parse_decimal_amount(&max, dec_value)
                .map(|v| v.to_string())
                .unwrap_or_else(|_| "0".to_string());
            let max_float = max_base_units.parse::<f64>().unwrap_or(0.0);
            let progress = if max_float == 0.0 {
                0.0
            } else {
                (supply_float / max_float).clamp(0.0, 1.0)
            };

            items.push(TokenSummary {
                ticker,
                max,
                max_base_units,
                supply: display_supply,
                supply_base_units,
                lim,
                dec,
                deployer,
                inscription_id,
                progress,
            });
        }
    }

    Ok(Json(PaginatedResponse {
        page,
        limit,
        total,
        has_more,
        items,
    }))
}

async fn get_names_feed(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<NameSummary>>, StatusCode> {
    let (page, limit) = params.resolve();
    let total = state.db.get_name_count().map_err(|err| {
        tracing::error!("name count error: {}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let rows = state.db.get_names_page(page, limit).map_err(|err| {
        tracing::error!("name page error: {}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let offset = (page as u64).saturating_mul(limit as u64);
    let has_more = offset + (rows.len() as u64) < total;

    let mut items = Vec::with_capacity(rows.len());
    for (_key, payload) in rows {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&payload) {
            let name = data["name"].as_str().unwrap_or("").to_string();
            let owner = data["owner"].as_str().unwrap_or("unknown").to_string();
            let inscription_id = data["inscription_id"].as_str().unwrap_or("").to_string();

            items.push(NameSummary {
                name,
                owner,
                inscription_id,
            });
        }
    }

    Ok(Json(PaginatedResponse {
        page,
        limit,
        total,
        has_more,
        items,
    }))
}
async fn get_inscription_preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let meta = match state.db.get_inscription(&id).unwrap_or(None) {
        Some(m) => m,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<h1>Inscription not found</h1>"),
            )
                .into_response()
        }
    };

    let val: serde_json::Value = match serde_json::from_str(&meta) {
        Ok(v) => v,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid metadata").into_response(),
    };

    let content_type = val["content_type"].as_str().unwrap_or("text/plain");
    let content_hex = val["content_hex"].as_str().unwrap_or("");
    let id_attr = html_escape::encode_double_quoted_attribute(&id).to_string();
    let title = html_escape::encode_text(&id).to_string();

    // Build HTML preview
    let preview_html = if content_type.starts_with("image/") {
        format!(
            r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>{}</title>
<style>body{{background:#111;margin:0;display:flex;align-items:center;justify-content:center;min-height:100vh;}}</style>
</head>
<body><img src="/content/{}" style="max-width:100%;max-height:100vh;"></body>
</html>"#,
            title, id_attr
        )
    } else if content_type == "text/html" {
        // Serve HTML directly via content route
        format!(
            r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>{}</title></head>
<body><iframe src="/content/{}" style="width:100%;height:100vh;border:none;"></iframe></body>
</html>"#,
            title, id_attr
        )
    } else if content_type.starts_with("text/") || content_type == "application/json" {
        let content_bytes = hex::decode(content_hex).unwrap_or_default();
        let text = String::from_utf8(content_bytes).unwrap_or_else(|_| "Invalid UTF-8".to_string());
        format!(
            r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>{}</title>
<style>body{{background:#111;color:#fff;font-family:monospace;padding:20px;line-height:1.6;}}pre{{white-space:pre-wrap;word-wrap:break-word;}}</style>
</head>
<body><pre>{}</pre></body>
</html>"#,
            title,
            html_escape::encode_text(&text)
        )
    } else {
        format!(
            r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>{}</title>
<style>body{{background:#111;color:#fff;font-family:monospace;padding:40px;text-align:center;}}</style>
</head>
<body><h2>Binary Content ({})</h2><a href="/content/{}" style="color:#fff;">Download</a></body>
</html>"#,
            title,
            html_escape::encode_text(content_type),
            id_attr
        )
    };

    Html(preview_html).into_response()
}

async fn get_block(
    State(_state): State<AppState>,
    Path(query): Path<String>,
) -> Json<serde_json::Value> {
    // Simple placeholder - would need RPC connection to get block details
    Json(serde_json::json!({
        "error": "Block explorer not yet implemented",
        "query": query
    }))
}

async fn get_transaction(
    State(_state): State<AppState>,
    Path(txid): Path<String>,
) -> Json<serde_json::Value> {
    // Simple placeholder - would need RPC connection to get tx details
    Json(serde_json::json!({
        "error": "Transaction explorer not yet implemented",
        "txid": txid
    }))
}

async fn get_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let height = state.db.get_latest_indexed_height().unwrap_or(None);
    let inscriptions = state.db.get_inscription_count().unwrap_or(0);
    let tokens = state.db.get_token_count().unwrap_or(0);
    let names = state.db.get_name_count().unwrap_or(0);

    Json(serde_json::json!({
        "height": height,
        "inscriptions": inscriptions,
        "tokens": tokens,
        "names": names,
        "synced": true,
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn api_docs() -> Html<String> {
    Html(r#"<!DOCTYPE html>
<html>
<head>
    <meta charset=\"utf-8\">
    <title>Zord API</title>
    <style>
        body { font-family: monospace; background: #111; color: #fff; padding: 40px; line-height: 1.6; }
        a { color: #6cf; }
        .card { max-width: 720px; margin: 0 auto; border: 1px solid #333; border-radius: 8px; padding: 24px; background: #1a1a1a; }
        code { background: #000; padding: 2px 6px; border-radius: 4px; }
    </style>
</head>
<body>
    <div class=\"card\">
        <h1>Zord API</h1>
        <p>Use the JSON endpoints that power the new component library:</p>
        <ul>
            <li><code>/api/v1/inscriptions?page=0&limit=24</code></li>
            <li><code>/api/v1/tokens?page=0&limit=100</code></li>
            <li><code>/api/v1/names?page=0&limit=100</code></li>
            <li><code>/api/v1/status</code></li>
        </ul>
        <p>Full documentation lives in <a href=\"https://github.com/zatoshi/zord/tree/main/docs\">/docs</a> inside the repository.</p>
        <p>Legacy ord-compatible routes such as <code>/inscription/:id</code> and <code>/content/:id</code> remain available for tooling parity.</p>
    </div>
</body>
</html>"#.to_string())
}

async fn get_all_tokens_api(State(state): State<AppState>) -> Json<serde_json::Value> {
    let tokens = state.db.get_all_tokens().unwrap_or_default();

    let mut token_list: Vec<serde_json::Value> = Vec::new();
    for (ticker, info_str) in tokens {
        if let Ok(mut info) = serde_json::from_str::<serde_json::Value>(&info_str) {
            info["ticker"] = serde_json::Value::String(ticker);

            // Format supply and max with proper decimal handling
            let dec = info["dec"]
                .as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(18);
            let divisor = 10u64.pow(dec) as f64;

            // Parse supply (stored as base units)
            let supply_str = info["supply"].as_str().unwrap_or("0");
            if let Ok(supply_base) = supply_str.parse::<u64>() {
                info["supply_display"] =
                    serde_json::json!((supply_base as f64 / divisor).to_string());
            }

            // Parse max (stored as human-readable, need to multiply for base units)
            let max_str = info["max"].as_str().unwrap_or("0");
            if let Ok(max_value) = parse_decimal_amount(max_str, dec) {
                info["max_display"] = serde_json::json!(max_str);
                info["max_base"] = serde_json::json!(max_value.to_string());
            }

            token_list.push(info);
        }
    }

    // Sort by inscription_id (reverse chronological order - newest first)
    token_list.sort_by(|a, b| {
        let id_a = a["inscription_id"].as_str().unwrap_or("");
        let id_b = b["inscription_id"].as_str().unwrap_or("");
        id_b.cmp(id_a) // Reversed for newest first
    });

    Json(serde_json::json!({
        "tokens": token_list
    }))
}

// Helper function to parse amounts with decimals
fn parse_decimal_amount(amount_str: &str, decimals: u32) -> Result<u64, std::num::ParseIntError> {
    if amount_str.contains('.') {
        let parts: Vec<&str> = amount_str.split('.').collect();
        let whole: u64 = parts[0].parse()?;
        let frac = if parts.len() > 1 { parts[1] } else { "0" };

        // Pad or truncate fractional part to match decimals
        let frac_padded = format!("{:0<width$}", frac, width = decimals as usize);
        let frac_value: u64 = frac_padded.parse()?;

        Ok(whole * 10u64.pow(decimals) + frac_value)
    } else {
        let whole: u64 = amount_str.parse()?;
        Ok(whole * 10u64.pow(decimals))
    }
}

fn build_preview(content_type: &str, value: &serde_json::Value) -> Option<String> {
    if content_type.starts_with("text/") || content_type == "application/json" {
        if let Some(body) = value["content"].as_str() {
            let snippet: String = body.chars().take(240).collect();
            if snippet.is_empty() {
                None
            } else {
                Some(snippet)
            }
        } else {
            None
        }
    } else {
        None
    }
}

fn format_decimal(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }

    let mut formatted = format!("{:.8}", value);
    while formatted.contains('.') && formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.pop();
    }

    if formatted.is_empty() {
        "0".to_string()
    } else {
        formatted
    }
}

// Names API handlers
async fn get_all_names_api(State(state): State<AppState>) -> Json<serde_json::Value> {
    let names = state.db.get_all_names().unwrap_or_default();

    let mut name_list: Vec<serde_json::Value> = Vec::new();
    for (_name_lower, data_str) in names {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&data_str) {
            name_list.push(data);
        }
    }

    // Sort by inscription_id (chronological order - first registered = first in list)
    name_list.sort_by(|a, b| {
        let id_a = a["inscription_id"].as_str().unwrap_or("");
        let id_b = b["inscription_id"].as_str().unwrap_or("");
        id_a.cmp(id_b)
    });

    Json(serde_json::json!({
        "names": name_list
    }))
}

async fn get_name_info(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Json<serde_json::Value> {
    let name_lower = name.to_lowercase();

    if let Ok(Some(data_str)) = state.db.get_name(&name_lower) {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&data_str) {
            return Json(data);
        }
    }

    Json(serde_json::json!({
        "error": "Name not found"
    }))
}

async fn resolve_name(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Json<serde_json::Value> {
    let name_lower = name.to_lowercase();

    if let Ok(Some(data_str)) = state.db.get_name(&name_lower) {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&data_str) {
            if let Some(owner) = data["owner"].as_str() {
                return Json(serde_json::json!({
                    "name": data["name"].as_str().unwrap_or(&name),
                    "address": owner
                }));
            }
        }
    }

    Json(serde_json::json!({
        "error": "Name not found"
    }))
}
