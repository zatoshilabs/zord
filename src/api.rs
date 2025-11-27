use crate::db::Db;
use crate::rpc::ZcashRpcClient;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use axum::middleware::{self, Next};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tower::BoxError;
use tower::ServiceBuilder;
use tower::limit::ConcurrencyLimitLayer;
use tower::timeout::TimeoutLayer;
use tower_http::cors::CorsLayer;
use tower_http::compression::CompressionLayer;
use axum::error_handling::HandleErrorLayer;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use std::fs;
use axum::body::Body;
use tower_http::services::ServeDir;

const FRONT_HTML: &str = include_str!("../web/index.html");
const MAX_PAGE_SIZE: usize = 50000;

#[derive(Deserialize)]
struct PaginationParams {
    page: Option<usize>,
    limit: Option<usize>,
    q: Option<String>,
    tld: Option<String>,
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
    metrics: Arc<ServerMetrics>,
}

#[derive(Default)]
pub struct ServerMetrics {
    inflight: AtomicUsize,
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
    block_time: Option<u64>,
    block_height: Option<u64>,
    content_length: usize,
    shielded: bool,
    category: String,
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
struct Zrc721CollectionSummary {
    collection: String,
    supply: String,
    minted: u64,
    meta: serde_json::Value,
    royalty: String,
    deployer: String,
    inscription_id: String,
}

#[derive(Serialize)]
struct Zrc721TokenSummary {
    tick: String,
    token_id: String,
    owner: String,
    inscription_id: String,
    metadata: serde_json::Value,
    metadata_path: Option<String>,
}

#[derive(Serialize)]
struct NameSummary {
    name: String,
    owner: String,
    inscription_id: String,
}

pub async fn start_api(db: Db, port: u16) {
    let metrics = Arc::new(ServerMetrics { inflight: AtomicUsize::new(0) });
    let state = AppState { db, metrics: metrics.clone() };

    // Runtime tunables: concurrency & request timeout
    let max_inflight: usize = std::env::var("API_MAX_INFLIGHT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2048);
    let timeout_secs: u64 = std::env::var("API_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);

    let middleware = ServiceBuilder::new()
        // Convert middleware errors (e.g., timeouts) into HTTP responses
        .layer(HandleErrorLayer::new(|err: BoxError| async move {
            if err.is::<tower::timeout::error::Elapsed>() {
                return (
                    axum::http::StatusCode::REQUEST_TIMEOUT,
                    "request timed out",
                )
                    .into_response();
            }
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("internal error: {}", err),
            )
                .into_response()
        }))
        .layer(TimeoutLayer::new(std::time::Duration::from_secs(timeout_secs)))
        .layer(ConcurrencyLimitLayer::new(max_inflight))
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new());

    let app = Router::new()
        // Static HTML entry points
        .route("/", get(frontpage))
        .route("/tokens", get(tokens_page))
        .route("/names", get(names_page))
        .route("/names/zec", get(names_zec_page))
        .route("/names/zcash", get(names_zcash_page))
        .route("/collections", get(collections_page))
        .route("/zrc721", get(collections_page))
        .route("/collection/:tick", get(collection_detail_page))
        .route("/docs", get(docs_page))
        .route("/spec", get(spec_page))
        .route("/api", get(api_docs))
        .route("/api/v1/metrics", get(get_metrics))
        // JSON feeds powering the frontend widgets
        .route("/api/v1/inscriptions", get(get_inscriptions_feed))
        .route("/api/v1/tokens", get(get_tokens_feed))
        .route("/api/v1/names", get(get_names_feed))
        .route("/api/v1/names/zec", get(get_names_feed_zec))
        .route("/api/v1/names/zcash", get(get_names_feed_zcash))
        .route("/api/v1/names/address/:address", get(get_names_by_address))
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/zrc20/status", get(get_zrc20_status))
        .route("/api/v1/zrc20/tokens", get(get_tokens_feed))
        .route("/api/v1/zrc20/token/:tick", get(get_token_info))
        .route(
            "/api/v1/zrc20/token/:tick/summary",
            get(get_zrc20_token_summary),
        )
        .route("/api/v1/zrc20/token/:tick/balances", get(get_zrc20_token_balances))
        .route("/api/v1/zrc20/address/:address", get(get_zrc20_address_balances))
        .route(
            "/api/v1/zrc20/token/:tick/rank/:address",
            get(get_zrc20_rank),
        )
        .route(
            "/api/v1/zrc20/token/:tick/integrity",
            get(get_zrc20_token_integrity),
        )
        .route("/api/v1/zrc20/transfer/:id", get(get_zrc20_transfer))
        .route("/api/v1/zrc721/status", get(get_zrc721_status))
        .route("/api/v1/zrc721/collections", get(get_zrc721_collections))
        .route("/api/v1/zrc721/collection/:tick", get(get_zrc721_collection))
        .route(
            "/api/v1/zrc721/collection/:tick/tokens",
            get(get_zrc721_collection_tokens),
        )
        .route("/api/v1/zrc721/address/:address", get(get_zrc721_address_tokens))
        .route(
            "/api/v1/zrc721/token/:collection/:id",
            get(get_zrc721_token_info),
        )
        .route("/api/v1/healthz", get(get_healthz))
        .route(
            "/api/v1/zrc20/token/:tick/burned",
            get(get_zrc20_burned),
        )
        // Compatibility endpoints for Ord-style tools
        .route("/inscription/:id", get(get_inscription))
        .route("/inscriptions", get(get_recent_inscriptions))
        .route("/content/:id", get(get_inscription_content))
        .route("/preview/:id", get(get_inscription_preview))
        .route("/block/:query", get(get_block))
        .route("/tx/:txid", get(get_transaction))
        .route("/status", get(get_status))
        // Misc helper endpoints
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
        .route("/api/v1/resolve/:name", get(resolve_name))
        // Static asset server (keep last)
        .nest_service("/static", ServeDir::new("web"))
        .layer(middleware)
        // Track in-flight requests for metrics
        .layer(middleware::from_fn_with_state(state.clone(), track_inflight))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("API listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn track_inflight(State(state): State<AppState>, req: axum::http::Request<Body>, next: Next) -> impl IntoResponse {
    state.metrics.inflight.fetch_add(1, Ordering::Relaxed);
    let res = next.run(req).await;
    state.metrics.inflight.fetch_sub(1, Ordering::Relaxed);
    res
}

async fn get_metrics(State(state): State<AppState>) -> Json<serde_json::Value> {
    let inflight = state.metrics.inflight.load(Ordering::Relaxed) as u64;
    let open_fds = count_open_fds();
    let (soft, hard) = get_fd_limits();
    Json(serde_json::json!({
        "inflight": inflight,
        "open_fds": open_fds,
        "limits": { "nofile": { "soft": soft, "hard": hard } }
    }))
}

fn count_open_fds() -> serde_json::Value {
    match fs::read_dir("/proc/self/fd") {
        Ok(rd) => serde_json::json!(rd.count()),
        Err(_) => serde_json::Value::Null,
    }
}

fn get_fd_limits() -> (serde_json::Value, serde_json::Value) {
    if let Ok(contents) = fs::read_to_string("/proc/self/limits") {
        for line in contents.lines() {
            if line.to_lowercase().contains("max open files") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 5 {
                    let soft = parts[3].parse::<u64>().ok();
                    let hard = parts[4].parse::<u64>().ok();
                    return (serde_json::json!(soft), serde_json::json!(hard));
                }
            }
        }
    }
    (serde_json::Value::Null, serde_json::Value::Null)
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
        None => {
            return Html(
                r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Inscription Not Found</title>
    <style>
        body { font-family: monospace; background: #020204; color: #fff; padding: 40px; text-align: center; }
        a { color: #ffc837; text-decoration: none; }
    </style>
</head>
<body>
    <h1>Inscription Not Found</h1>
    <a href="/">← Back to index</a>
</body>
</html>"#
                .to_string(),
            )
            .into_response()
        }
    };

    let val: serde_json::Value = match serde_json::from_str(&meta) {
        Ok(v) => v,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid metadata").into_response(),
    };

    let content_type_raw = val["content_type"].as_str().unwrap_or("text/plain");
    let content = val["content"].as_str().unwrap_or("");
    let content_hex = val["content_hex"].as_str().unwrap_or("");
    let sender_raw = val["sender"].as_str().unwrap_or("unknown");
    let receiver_raw = val["receiver"].as_str().unwrap_or("unknown");
    let txid_raw = val["txid"].as_str().unwrap_or("");
    let block_height = val["block_height"].as_u64();
    let block_time = val["block_time"].as_u64();

    let sender = html_escape::encode_text(sender_raw).to_string();
    let receiver = html_escape::encode_text(receiver_raw).to_string();
    let txid = html_escape::encode_text(txid_raw).to_string();
    let content_type = html_escape::encode_text(content_type_raw).to_string();
    let id_text = html_escape::encode_text(&id).to_string();
    let id_attr = html_escape::encode_double_quoted_attribute(&id).to_string();
    let short_id: String = id_text.chars().take(16).collect();
    let content_length_bytes = content_hex.len() / 2;
    let size_display = format_byte_size(content_length_bytes);
    let timestamp_display = block_time.map(format_timestamp).unwrap_or_else(|| "—".into());
    let category = classify_mime(content_type_raw);
    let content_encoding = val["content_encoding"].as_str().map(|s| s.to_string());

    let content_preview = if content_type_raw.starts_with("image/") {
        let rendering = if matches!(content_type_raw, "image/avif" | "image/jxl") {
            "auto"
        } else {
            "pixelated"
        };

        format!(
            r#"<div class=\"preview-box\"><img src=\"/content/{id}\" alt=\"{short}\" loading=\"lazy\" style=\"image-rendering:{rendering};\"></div>"#,
            id = id_attr,
            short = short_id,
            rendering = rendering,
        )
    } else if content_type_raw == "text/html" {
        format!(
            r#"<div class=\"preview-box\"><iframe src=\"/content/{id}\" title=\"{short}\" loading=\"lazy\"></iframe></div>"#,
            id = id_attr,
            short = short_id,
        )
    } else if content_type_raw.starts_with("text/") || content_type_raw == "application/json" {
        let formatted = if content_type_raw == "application/json" {
            serde_json::from_str::<serde_json::Value>(content)
                .ok()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| content.to_string())
        } else {
            content.to_string()
        };

        format!(
            r#"<div class=\"preview-box\"><pre>{}</pre></div>"#,
            html_escape::encode_text(&formatted)
        )
    } else {
        format!(
            r#"<div class=\"preview-box\"><div>Binary ({})</div></div>"#,
            size_display
        )
    };

    let block_link = block_height
        .map(|h| format!("<a href=\"/block/{h}\">{h}</a>"))
        .unwrap_or_else(|| "—".into());
    let tx_link = if txid_raw.is_empty() {
        "—".to_string()
    } else {
        format!("<a href=\"/tx/{tx}\">{tx}</a>", tx = txid)
    };
    let preview_link = format!("<a href=\"/preview/{id}\" target=\"_blank\" rel=\"noreferrer\">Open preview</a>", id = id_attr);
    let content_link = format!("<a href=\"/content/{id}\" target=\"_blank\" rel=\"noreferrer\">Download raw</a>", id = id_attr);

    let mut rows = Vec::new();
    rows.push(format!("<dt>ID</dt><dd><code>{}</code></dd>", id_text));
    rows.push(format!("<dt>Content type</dt><dd>{}</dd>", content_type));
    if let Some(enc) = content_encoding {
        rows.push(format!("<dt>Encoding</dt><dd>{}</dd>", enc));
    }
    rows.push(format!("<dt>Category</dt><dd>{}</dd>", category.to_uppercase()));
    rows.push(format!("<dt>Size</dt><dd>{}</dd>", size_display));
    rows.push(format!("<dt>Sender</dt><dd><code>{}</code></dd>", sender));
    rows.push(format!("<dt>Receiver</dt><dd><code>{}</code></dd>", receiver));
    rows.push(format!("<dt>Block height</dt><dd>{}</dd>", block_link));
    rows.push(format!("<dt>Timestamp</dt><dd>{}</dd>", timestamp_display));
    rows.push(format!("<dt>Transaction</dt><dd>{}</dd>", tx_link));
    rows.push(format!("<dt>Preview</dt><dd>{}</dd>", preview_link));
    rows.push(format!("<dt>Content</dt><dd>{}</dd>", content_link));
    let meta_rows = rows.join("\n");

    let html = format!(
        r#"<!DOCTYPE html>
<html lang=\"en\">
<head>
    <meta charset=\"utf-8\">
    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
    <title>Inscription {short}</title>
    <link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">
    <link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin>
    <link href=\"https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600&display=swap\" rel=\"stylesheet\">
    <link rel=\"stylesheet\" href=\"/static/styles.css\">
</head>
<body class=\"inscription-page\">
    <header class=\"bar\">
        <nav>
            <a href=\"/\" class=\"active\">inscriptions</a>
            <a href=\"/tokens\">zrc-20</a>
            <a href=\"/names\">names</a>
            <a href=\"/docs\">docs</a>
            <a href=\"/spec\">api</a>
        </nav>
        <zord-status></zord-status>
    </header>

    <main class=\"inscription-main\">
        <section class=\"inscription-preview\">
            {preview}
        </section>
        <section class=\"inscription-meta\">
            <dl class=\"meta-grid\">
            {rows}
            </dl>
        </section>
    </main>

    <sync-footer></sync-footer>
    <script type=\"module\" src=\"/static/app.js\"></script>
</body>
</html>"#,
        short = short_id,
        preview = content_preview,
        rows = meta_rows
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

    // Materialize stored hex payload
    let content_bytes = match hex::decode(content_hex) {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid content data").into_response()
        }
    };

    // Preserve original MIME type
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
    // Lookup inscription by ordinal number

    let id = state.db.get_inscription_by_number(number).unwrap_or(None);
    if let Some(inscription_id) = id {
        // Embed the resolved id/number in the JSON blob
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

async fn get_zrc20_token_summary(
    State(state): State<AppState>,
    Path(tick): Path<String>,
) -> impl IntoResponse {
    let lower = tick.to_lowercase();
    let token_info = state.db.get_token_info(&lower).unwrap_or(None);
    if let Some(raw) = token_info {
        if let Ok(info) = serde_json::from_str::<serde_json::Value>(&raw) {
            let dec = info["dec"].as_str().unwrap_or("18");
            let supply_base = info["supply"].as_str().unwrap_or("0").to_string();
            let max = info["max"].as_str().unwrap_or("0");
            let lim = info["lim"].as_str().unwrap_or("");
            let (sum_overall, _sum_avail, holders_total, holders_positive) =
                state.db.sum_balances_for_tick(&lower).unwrap_or((0, 0, 0, 0));
            let transfers_completed = state
                .db
                .count_completed_transfers_for_tick(&lower)
                .unwrap_or(0);
            let burned = state.db.get_burned(&lower).unwrap_or(0);
            let consistent = parse_u128(&supply_base) == sum_overall + burned;
            let body = serde_json::json!({
                "tick": lower,
                "dec": dec,
                "supply_base_units": supply_base,
                // Report holders as positive-balance addresses; also include total rows for transparency
                "holders": holders_positive,
                "holders_total": holders_total,
                "transfers_completed": transfers_completed,
                "max": max,
                "lim": lim,
                "integrity": { "consistent": consistent, "sum_holders_base_units": sum_overall.to_string(), "burned_base_units": burned.to_string() }
            });
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("public, max-age=10"));
            return (headers, Json(body));
        }
    }
    {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("public, max-age=10"));
        (headers, Json(serde_json::json!({ "error": "Not found" })))
    }
}

async fn get_zrc20_rank(
    State(state): State<AppState>,
    Path((tick, address)): Path<(String, String)>,
) -> Json<serde_json::Value> {
    let (rank, total) = state
        .db
        .rank_for_address_in_tick(&tick, &address)
        .unwrap_or((0, 0));
    let percentile = if total == 0 || rank == 0 {
        0.0
    } else {
        // Higher balance = better (lower) rank; percentile as top share
        let r = rank as f64;
        let t = total as f64;
        (1.0 - (r - 1.0) / t) * 100.0
    };
    Json(serde_json::json!({
        "tick": tick,
        "address": address,
        "rank": rank,
        "total_holders": total,
        "percentile": percentile
    }))
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

async fn get_zrc20_token_balances(
    State(state): State<AppState>,
    Path(tick): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Json<serde_json::Value> {
    let (page, limit) = params.resolve();
    let (rows, total) = state
        .db
        .list_balances_for_tick(&tick, page, limit)
        .unwrap_or_default();
    let holders: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(address, bal)| {
            serde_json::json!({
                "address": address,
                "available": bal.available.to_string(),
                "overall": bal.overall.to_string(),
            })
        })
        .collect();
    Json(serde_json::json!({
        "tick": tick,
        "page": page,
        "limit": limit,
        "total_holders": total,
        "holders": holders
    }))
}

async fn get_zrc20_address_balances(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Json<serde_json::Value> {
    let rows = state
        .db
        .list_balances_for_address(&address)
        .unwrap_or_default();
    let entries: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(tick, bal)| {
            serde_json::json!({
                "tick": tick,
                "available": bal.available.to_string(),
                "overall": bal.overall.to_string(),
            })
        })
        .collect();
    Json(serde_json::json!({
        "address": address,
        "balances": entries
    }))
}

async fn get_zrc20_transfer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    if let Some(raw) = state.db.get_transfer_inscription(&id).unwrap_or(None) {
        let used = state.db.is_inscription_used(&id).unwrap_or(false);
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        let outpoint = state.db.find_outpoint_by_transfer_id(&id).unwrap_or(None);
        return Json(serde_json::json!({
            "inscription_id": id,
            "transfer": parsed,
            "used": used,
            "outpoint": outpoint
        }));
    }
    Json(serde_json::json!({ "error": "Transfer not found" }))
}

async fn get_zrc20_token_integrity(
    State(state): State<AppState>,
    Path(tick): Path<String>,
) -> impl IntoResponse {
    let lower = tick.to_lowercase();
    let token_info = state.db.get_token_info(&lower).unwrap_or(None);
    if let Some(info_str) = token_info {
        if let Ok(info) = serde_json::from_str::<serde_json::Value>(&info_str) {
            let supply_base = info["supply"]
                .as_str()
                .unwrap_or("0")
                .to_string();
            let dec = info["dec"].as_str().unwrap_or("18");
            let (sum_overall, sum_available, holders_total, holders_positive) =
                state.db.sum_balances_for_tick(&lower).unwrap_or((0, 0, 0, 0));
            let burned = state.db.get_burned(&lower).unwrap_or(0);
            let supply = parse_u128(&supply_base);
            let consistent = supply == sum_overall + burned;
            let body = serde_json::json!({
                "tick": lower,
                "dec": dec,
                "supply_base_units": supply_base,
                "sum_overall_base_units": sum_overall.to_string(),
                "sum_available_base_units": sum_available.to_string(),
                "total_holders": holders_total,
                "holders_positive": holders_positive,
                "burned_base_units": burned.to_string(),
                "consistent": consistent
            });
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("public, max-age=10"));
            return (headers, Json(body));
        }
    }
    {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("public, max-age=10"));
        (headers, Json(serde_json::json!({ "error": "Token not found" })))
    }
}

async fn get_zrc721_collections(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Json<serde_json::Value> {
    let (page, limit) = params.resolve();
    let rows = state
        .db
        .list_zrc721_collections(page, limit)
        .unwrap_or_default();
    let items: Vec<Zrc721CollectionSummary> = rows
        .into_iter()
        .filter_map(|(_tick, raw)| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .map(|info| Zrc721CollectionSummary {
            collection: info["collection"].as_str().unwrap_or("").to_string(),
            supply: info["supply"].as_str().unwrap_or("0").to_string(),
            minted: info["minted"].as_u64().unwrap_or(0),
            meta: info.get("meta").cloned().unwrap_or(serde_json::json!(null)),
            royalty: info["royalty"].as_str().unwrap_or("").to_string(),
            deployer: info["deployer"].as_str().unwrap_or("").to_string(),
            inscription_id: info["inscription_id"].as_str().unwrap_or("").to_string(),
        })
        .collect();
    Json(serde_json::json!({
        "page": page,
        "limit": limit,
        "collections": items
    }))
}

async fn get_zrc721_collection(
    State(state): State<AppState>,
    Path(tick): Path<String>,
) -> Json<serde_json::Value> {
    if let Some(raw) = state.db.get_zrc721_collection(&tick).unwrap_or(None) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
            return Json(val);
        }
    }
    Json(serde_json::json!({ "error": "Collection not found" }))
}

async fn get_zrc721_collection_tokens(
    State(state): State<AppState>,
    Path(tick): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Json<serde_json::Value> {
    let (page, limit) = params.resolve();
    let rows = state
        .db
        .list_zrc721_tokens(&tick, page, limit)
        .unwrap_or_default();
    // Try to fetch collection meta (CID) to derive metadata path
    let meta_cid = state
        .db
        .get_zrc721_collection(&tick)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v["meta"].as_str().map(|s| s.to_string()));

    let tokens: Vec<Zrc721TokenSummary> = rows
        .into_iter()
        .map(|token| {
            let metadata_path = meta_cid
                .as_ref()
                .map(|cid| format!("ipfs://{}/{}.json", cid, token.token_id));
            Zrc721TokenSummary {
                tick: token.tick,
                token_id: token.token_id,
                owner: token.owner,
                inscription_id: token.inscription_id,
                metadata: token.metadata,
                metadata_path,
            }
        })
        .collect();
    Json(serde_json::json!({
        "tick": tick,
        "page": page,
        "limit": limit,
        "tokens": tokens
    }))
}

async fn get_zrc721_address_tokens(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Json<serde_json::Value> {
    let (page, limit) = params.resolve();
    let rows = state
        .db
        .list_zrc721_tokens_by_address(&address, page, limit)
        .unwrap_or_default();
    // Derive metadata path if meta CID is available for each token's collection
    let tokens: Vec<Zrc721TokenSummary> = rows
        .into_iter()
        .map(|token| {
            let meta_cid = state
                .db
                .get_zrc721_collection(&token.tick)
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .and_then(|v| v["meta"].as_str().map(|s| s.to_string()));
            let metadata_path = meta_cid
                .as_ref()
                .map(|cid| format!("ipfs://{}/{}.json", cid, token.token_id));
            Zrc721TokenSummary {
                tick: token.tick,
                token_id: token.token_id,
                owner: token.owner,
                inscription_id: token.inscription_id,
                metadata: token.metadata,
                metadata_path,
            }
        })
        .collect();
    Json(serde_json::json!({
        "address": address,
        "page": page,
        "limit": limit,
        "tokens": tokens
    }))
}

async fn get_zrc721_token_info(
    State(state): State<AppState>,
    Path((collection, id)): Path<(String, String)>,
) -> Json<serde_json::Value> {
    let lower = collection.to_lowercase();
    if let Ok(Some(raw)) = state.db.get_zrc721_token(&lower, &id) {
        if let Ok(mut token) = serde_json::from_str::<serde_json::Value>(&raw) {
            let meta_cid = state
                .db
                .get_zrc721_collection(&lower)
                .ok()
                .flatten()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v["meta"].as_str().map(|s| s.to_string()));
            if let Some(cid) = meta_cid {
                token["metadata_path"] = serde_json::json!(format!("ipfs://{}/{}.json", cid, id));
            }
            return Json(token);
        }
    }
    Json(serde_json::json!({ "error": "Token not found" }))
}

async fn get_zrc20_burned(
    State(state): State<AppState>,
    Path(tick): Path<String>,
) -> Json<serde_json::Value> {
    let lower = tick.to_lowercase();
    let burned = state.db.get_burned(&lower).unwrap_or(0);
    Json(serde_json::json!({ "tick": lower, "burned_base_units": burned.to_string() }))
}

async fn get_healthz(State(state): State<AppState>) -> Json<serde_json::Value> {
    let height = state.db.get_latest_indexed_height().unwrap_or(None);
    let chain_tip = state.db.get_status("chain_tip").unwrap_or(None);
    let zrc20_height = state.db.get_status("zrc20_height").unwrap_or(None);
    let zrc721_height = state.db.get_status("zrc721_height").unwrap_or(None);
    let names_height = state.db.get_status("names_height").unwrap_or(None);
    let synced = match (height, chain_tip) { (Some(h), Some(t)) => h >= t.saturating_sub(1), _ => false };
    Json(serde_json::json!({
        "height": height,
        "chain_tip": chain_tip,
        "components": {
            "zrc20": { "height": zrc20_height, "tip": chain_tip },
            "zrc721": { "height": zrc721_height, "tip": chain_tip },
            "names": { "height": names_height, "tip": chain_tip }
        },
        "synced": synced,
        "version": env!("CARGO_PKG_VERSION")
    }))
}

// Minimal HTML shells used by browsers

async fn frontpage() -> Html<&'static str> {
    Html(FRONT_HTML)
}

async fn tokens_page() -> Html<String> {
    match std::fs::read_to_string("web/tokens.html") {
        Ok(content) => Html(content),
        Err(_) => Html("<p>tokens page missing</p>".to_string()),
    }
}

async fn names_page() -> Html<String> {
    match std::fs::read_to_string("web/names.html") {
        Ok(content) => Html(content),
        Err(_) => Html("<p>names page missing</p>".to_string()),
    }
}

async fn names_zec_page() -> Html<String> {
    match std::fs::read_to_string("web/names_zec.html") {
        Ok(content) => Html(content),
        Err(_) => Html("<p>names .zec page missing</p>".to_string()),
    }
}

async fn names_zcash_page() -> Html<String> {
    match std::fs::read_to_string("web/names_zcash.html") {
        Ok(content) => Html(content),
        Err(_) => Html("<p>names .zcash page missing</p>".to_string()),
    }
}

async fn collections_page() -> Html<String> {
    match std::fs::read_to_string("web/collections.html") {
        Ok(content) => Html(content),
        Err(_) => Html("<p>collections page missing</p>".to_string()),
    }
}

async fn collection_detail_page(Path(_tick): Path<String>) -> Html<String> {
    match std::fs::read_to_string("web/collection.html") {
        Ok(content) => Html(content),
        Err(_) => Html("<p>collection page missing</p>".to_string()),
    }
}

async fn docs_page() -> Html<String> {
    match std::fs::read_to_string("web/docs.html") {
        Ok(content) => Html(content),
        Err(_) => Html("<p>docs page missing</p>".to_string()),
    }
}

async fn spec_page() -> Html<String> {
    match std::fs::read_to_string("web/spec.html") {
        Ok(content) => Html(content),
        Err(_) => Html("<p>spec page missing</p>".to_string()),
    }
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
        let block_time = parsed["block_time"].as_u64();
        let block_height = parsed["block_height"].as_u64();
        let content_length = parsed["content_hex"]
            .as_str()
            .map(|hex| hex.len() / 2)
            .unwrap_or(0);
        let shielded = parsed["sender"].as_str().map(|addr| addr.starts_with('z')).unwrap_or(false);
        let category = classify_mime(&content_type).to_string();
        let preview_text = build_preview(&content_type, &parsed);

        items.push(InscriptionSummary {
            id,
            content_type,
            sender,
            txid,
            block_time,
            block_height,
            content_length,
            shielded,
            category,
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

// Convenience filters for TLD-specific name feeds
async fn get_names_feed_zec(
    State(state): State<AppState>,
    Query(mut params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<NameSummary>>, StatusCode> {
    params.tld = Some("zec".to_string());
    get_names_feed(State(state), Query(params)).await
}

async fn get_names_feed_zcash(
    State(state): State<AppState>,
    Query(mut params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<NameSummary>>, StatusCode> {
    params.tld = Some("zcash".to_string());
    get_names_feed(State(state), Query(params)).await
}

async fn get_names_by_address(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Json<serde_json::Value> {
    let all = state.db.get_all_names().unwrap_or_default();
    let mut names = Vec::new();
    for (_name, data_str) in all {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data_str) {
            if val["owner"].as_str().map(|s| s == address).unwrap_or(false) {
                names.push(val);
            }
        }
    }
    Json(serde_json::json!({ "address": address, "names": names }))
}

async fn get_tokens_feed(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<TokenSummary>>, StatusCode> {
    let (page, limit) = params.resolve();
    
    let (rows, total) = if let Some(query) = &params.q {
        if query.trim().is_empty() {
             let total = state.db.get_token_count().unwrap_or(0);
             let rows = state.db.get_tokens_page(page, limit).unwrap_or_default();
             (rows, total)
        } else {
            let rows = state.db.search_tokens(query, 100).unwrap_or_default();
            let total = rows.len() as u64;
            (rows, total)
        }
    } else {
        let total = state.db.get_token_count().map_err(|err| {
            tracing::error!("token count error: {}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let rows = state.db.get_tokens_page(page, limit).map_err(|err| {
            tracing::error!("token page error: {}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        (rows, total)
    };

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
            let display_supply = format_supply_string(&supply_base_units, dec_value);
            let max_base_units = parse_decimal_amount(&max, dec_value)
                .map(|v| v.to_string())
                .unwrap_or_else(|_| "0".to_string());
            let max_units = parse_u128(&max_base_units);
            let supply_units = parse_u128(&supply_base_units);
            let progress = if max_units == 0 {
                0.0
            } else {
                (supply_units as f64 / max_units as f64).clamp(0.0, 1.0)
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

    // Pull all names and filter by optional tld and query for correctness
    let names_all = match state.db.get_all_names() {
        Ok(v) => v,
        Err(err) => {
            // During heavy reindexing, prefer a graceful empty result over a 500
            tracing::warn!("names fetch error (returning empty set): {}", err);
            Vec::new()
        }
    };

    let tld = params.tld.as_ref().map(|s| s.to_lowercase());
    let q_lower = params.q.as_ref().map(|s| s.to_lowercase());
    let mut filtered: Vec<NameSummary> = Vec::new();
    for (_key, payload) in names_all {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&payload) {
            let name = data["name"].as_str().unwrap_or("").to_string();
            // tld filter
            let keep_tld = match tld.as_deref() {
                Some("zec") => name.ends_with(".zec"),
                Some("zcash") => name.ends_with(".zcash"),
                _ => true,
            };
            if !keep_tld { continue; }
            // search filter
            if let Some(q) = &q_lower {
                if !name.to_lowercase().contains(q) { continue; }
            }
            let owner = data["owner"].as_str().unwrap_or("unknown").to_string();
            let inscription_id = data["inscription_id"].as_str().unwrap_or("").to_string();
            filtered.push(NameSummary { name, owner, inscription_id });
        }
    }
    // keep newest first by insertion order proxy
    filtered.reverse();
    let total = filtered.len() as u64;
    let start = page.saturating_mul(limit);
    let items: Vec<NameSummary> = filtered.into_iter().skip(start).take(limit).collect();
    let has_more = (start as u64) + (items.len() as u64) < total;

    Ok(Json(PaginatedResponse { page, limit, total, has_more, items }))
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

    // Derive an inline preview depending on MIME type
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
        // Wrap HTML inscriptions in an iframe so we sandbox execution
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
    let rpc = ZcashRpcClient::new();
    // Accept either height (u64) or hash
    let result = if let Ok(height) = query.parse::<u64>() {
        match rpc.get_block_hash(height).await {
            Ok(hash) => rpc.get_block(&hash).await.map(|blk| (hash, blk)),
            Err(e) => Err(e),
        }
    } else {
        let hash = query.clone();
        rpc.get_block(&hash).await.map(|blk| (hash, blk))
    };

    match result {
        Ok((hash, blk)) => Json(serde_json::json!({
            "hash": hash,
            "height": blk.height,
            "time": blk.time,
            "tx": blk.tx,
            "previous": blk.previousblockhash
        })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string(), "query": query })),
    }
}

async fn get_transaction(
    State(_state): State<AppState>,
    Path(txid): Path<String>,
) -> Json<serde_json::Value> {
    let rpc = ZcashRpcClient::new();
    match rpc.get_raw_transaction(&txid).await {
        Ok(tx) => {
            let vins: Vec<serde_json::Value> = tx
                .vin
                .into_iter()
                .map(|v| serde_json::json!({
                    "txid": v.txid,
                    "vout": v.vout
                }))
                .collect();
            let vouts: Vec<serde_json::Value> = tx
                .vout
                .into_iter()
                .map(|o| serde_json::json!({
                    "n": o.n,
                    "value": o.value,
                    "addresses": o.script_pub_key.addresses
                }))
                .collect();
            Json(serde_json::json!({
                "txid": tx.txid,
                "hex": tx.hex,
                "vin": vins,
                "vout": vouts
            }))
        }
        Err(e) => Json(serde_json::json!({ "error": e.to_string(), "txid": txid })),
    }
}

async fn get_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let height = state.db.get_latest_indexed_height().unwrap_or(None);
    let inscriptions = state.db.get_inscription_count().unwrap_or(0);
    let tokens = state.db.get_token_count().unwrap_or(0);
    let names = state.db.get_name_count().unwrap_or(0);
    let chain_tip = state.db.get_status("chain_tip").unwrap_or(None);
    let zrc20_height = state.db.get_status("zrc20_height").unwrap_or(None);
    let names_height = state.db.get_status("names_height").unwrap_or(None);

    Json(serde_json::json!({
        "height": height,
        "inscriptions": inscriptions,
        "tokens": tokens,
        "names": names,
        "synced": true,
        "version": env!("CARGO_PKG_VERSION"),
        "chain_tip": chain_tip,
        "components": {
            "core": { "height": height, "tip": chain_tip },
            "zrc20": { "height": zrc20_height, "tip": chain_tip },
            "names": { "height": names_height, "tip": chain_tip },
        }
    }))
}

async fn get_zrc20_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let height = state.db.get_status("zrc20_height").unwrap_or(None);
    let chain_tip = state.db.get_status("chain_tip").unwrap_or(None);
    let tokens = state.db.get_token_count().unwrap_or(0);
    Json(serde_json::json!({
        "height": height,
        "chain_tip": chain_tip,
        "tokens": tokens,
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn get_zrc721_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let (collections, tokens) = state.db.zrc721_counts().unwrap_or((0, 0));
    let height = state.db.get_status("zrc721_height").unwrap_or(None);
    let chain_tip = state.db.get_status("chain_tip").unwrap_or(None);
    Json(serde_json::json!({
        "collections": collections,
        "tokens": tokens,
        "height": height,
        "chain_tip": chain_tip,
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

            // Normalize supply/max based on decimals stored on-chain
            let dec = info["dec"]
                .as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(18);
            let divisor = 10u64.pow(dec) as f64;

            // Supply is persisted in base units
            let supply_str = info["supply"].as_str().unwrap_or("0");
            if let Ok(supply_base) = supply_str.parse::<u128>() {
                info["supply_display"] =
                    serde_json::json!((supply_base as f64 / divisor).to_string());
            }

            // Max field is human readable; convert to base units for comparison
            let max_str = info["max"].as_str().unwrap_or("0");
            if let Ok(max_value) = parse_decimal_amount(max_str, dec) {
                info["max_display"] = serde_json::json!(max_str);
                info["max_base"] = serde_json::json!(max_value.to_string());
            }

            token_list.push(info);
        }
    }

    // Order newest-first by inscription id (ids encode creation order)
    token_list.sort_by(|a, b| {
        let id_a = a["inscription_id"].as_str().unwrap_or("");
        let id_b = b["inscription_id"].as_str().unwrap_or("");
        id_b.cmp(id_a) // Keep newest entries at the top
    });

    Json(serde_json::json!({
        "tokens": token_list
    }))
}

// Parse a human-readable quantity into base units respecting decimals
fn parse_decimal_amount(amount_str: &str, decimals: u32) -> Result<u128, std::num::ParseIntError> {
    if amount_str.contains('.') {
        let parts: Vec<&str> = amount_str.split('.').collect();
        let whole: u128 = parts[0].parse()?;
        let frac = if parts.len() > 1 { parts[1] } else { "0" };

        // Clamp fractional digits to declared precision
        let frac_truncated = if frac.len() > decimals as usize {
            &frac[..decimals as usize]
        } else {
            frac
        };

        // Pad right side so we can treat the value as an integer
        let frac_padded = format!("{:0<width$}", frac_truncated, width = decimals as usize);
        let frac_value: u128 = frac_padded.parse()?;

        Ok(whole * 10u128.pow(decimals) + frac_value)
    } else {
        let whole: u128 = amount_str.parse()?;
        Ok(whole * 10u128.pow(decimals))
    }
}

fn format_byte_size(bytes: usize) -> String {
    const UNITS: [&str; 4] = ["bytes", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{:.0} {}", size, UNITS[unit])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}

fn format_timestamp(ts: u64) -> String {
    if let Some(datetime) = DateTime::<Utc>::from_timestamp(ts as i64, 0) {
        datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string()
    } else {
        ts.to_string()
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

fn format_supply_string(base_units: &str, decimals: u32) -> String {
    let value = parse_u128(base_units);
    if decimals == 0 {
        return value.to_string();
    }
    let scale = 10u128.pow(decimals);
    let whole = value / scale;
    let frac = value % scale;
    if frac == 0 {
        return whole.to_string();
    }
    let mut frac_str = format!("{:0width$}", frac, width = decimals as usize);
    while frac_str.ends_with('0') {
        frac_str.pop();
    }
    if frac_str.is_empty() {
        whole.to_string()
    } else {
        format!("{}.{}", whole, frac_str)
    }
}

fn parse_u128(value: &str) -> u128 {
    value.parse::<u128>().unwrap_or(0)
}

fn classify_mime(content_type: &str) -> &'static str {
    let lower = content_type.to_lowercase();
    if lower == "image/png" {
        "png"
    } else if lower == "image/jpeg" || lower == "image/jpg" {
        "jpeg"
    } else if lower == "image/gif" {
        "gif"
    } else if lower == "image/svg+xml" {
        "svg"
    } else if lower == "text/html" || lower == "application/xhtml+xml" {
        "html"
    } else if lower == "text/javascript" || lower == "application/javascript" {
        "javascript"
    } else if lower.starts_with("text/") {
        "text"
    } else if lower.starts_with("audio/") {
        "audio"
    } else if lower.starts_with("video/") {
        "video"
    } else if lower.starts_with("model/") {
        "3d"
    } else if lower.starts_with("image/") {
        "image"
    } else {
        "binary"
    }
}

// ZNS helper endpoints
async fn get_all_names_api(State(state): State<AppState>) -> Json<serde_json::Value> {
    let names = state.db.get_all_names().unwrap_or_default();

    let mut name_list: Vec<serde_json::Value> = Vec::new();
    for (_name_lower, data_str) in names {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&data_str) {
            name_list.push(data);
        }
    }

    // Preserve mint order (inscription_id encodes creation sequence)
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
