// ============================================================================
//  mcpub  v0.1.0
//  The open MCP endpoint directory. No gatekeepers.
//
//  WHAT IT IS:
//    A public directory of MCP endpoints. Submit via MCP. Discover via MCP.
//    Runs behind Caddy. No database — state lives in endpoints.csv.
//
//  WHAT IT IS NOT:
//    A curator, a gatekeeper, a security scanner.
//    If an endpoint is malicious that is the caller's problem.
//
//  ROUTES:
//    POST /mcp  — MCP endpoint for agents: submit | search | list_all
//
//  STORAGE:
//    /var/lib/mcpub/endpoints.csv — "url","description","trusted","submitted_at"
//
//  STATIC HTML:
//    /var/www/mcpub/index.html — served by Caddy, not this binary
//
//  CADDY:
//    mcpub.dev {
//        root * /var/www/mcpub
//        file_server
//        handle /mcp {
//            reverse_proxy localhost:3100
//        }
//    }
// ============================================================================

use std::{sync::Arc, time::Duration};

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    net::TcpListener,
    sync::RwLock,
};
use tracing::{error, info, warn};

// ============================================================================
// CONFIG — hardcoded absolute paths for production
// ============================================================================

const LISTEN_ADDR: &str = "127.0.0.1:3100";
const DATA_FILE: &str = "/var/lib/mcpub/endpoints.csv";
const CHECK_TIMEOUT_S: u64 = 5;
const WELL_KNOWN_PATH: &str = "/.well-known/mcp.json";

// ============================================================================
// DATA MODEL
// ============================================================================

#[derive(Clone, Serialize, Deserialize)]
struct Endpoint {
    url:          String,
    description:  String,
    trusted:      bool,
    submitted_at: i64,
}

// ============================================================================
// CSV STORAGE with sanitization
// ============================================================================

fn sanitize_description(input: &str) -> String {
    input
        .chars()
        .filter(|c| {
            match *c {
                '\x00'..='\x08' | '\x0b'..='\x0c' | '\x0e'..='\x1f' | '\x7f' => false,
                _ => true,
            }
        })
        .collect::<String>()
        .replace('"', "'")
        .replace('\n', " ")
        .replace('\r', " ")
        .replace('\t', " ")
        .trim()
        .to_string()
}

fn sanitize_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    
    if !trimmed.starts_with("https://") {
        return Err("url must use https://".to_string());
    }
    
    let cleaned = trimmed.split('?').next().unwrap_or(trimmed);
    let cleaned = cleaned.split('#').next().unwrap_or(cleaned);
    let cleaned = cleaned.trim_end_matches('/');
    
    let domain = cleaned.strip_prefix("https://").unwrap_or(cleaned);
    if domain.is_empty() || !domain.contains('.') {
        return Err("invalid domain".to_string());
    }
    
    Ok(cleaned.to_string())
}

fn load_csv(path: &str) -> Vec<Endpoint> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            info!("no existing CSV at {}, starting empty", path);
            return vec![];
        }
    };

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(content.as_bytes());
    
    let mut endpoints = Vec::new();
    
    for result in rdr.records() {
        match result {
            Ok(record) => {
                if record.len() < 4 { continue; }
                
                let url = record.get(0).unwrap_or("").trim();
                let description = record.get(1).unwrap_or("").trim();
                let trusted = record.get(2).unwrap_or("0").trim() == "1";
                let submitted_at = record.get(3).unwrap_or("0").trim().parse().unwrap_or(0);
                
                if url.is_empty() { continue; }
                
                endpoints.push(Endpoint {
                    url: url.to_string(),
                    description: description.to_string(),
                    trusted,
                    submitted_at,
                });
            }
            Err(e) => {
                error!("csv parse error: {e}");
            }
        }
    }
    
    endpoints
}

fn save_csv(path: &str, endpoints: &[Endpoint]) {
    // Ensure directory exists
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    
    let tmp = format!("{path}.tmp");
    
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(true)
        .quote_style(csv::QuoteStyle::Necessary)
        .from_path(&tmp)
        .expect("cannot create csv writer");
    
    wtr.write_record(&["url", "description", "trusted", "submitted_at"])
        .expect("cannot write header");
    
    for ep in endpoints {
        let trusted_str = if ep.trusted { "1" } else { "0" };
        let _ = wtr.write_record(&[
            &ep.url,
            &ep.description,
            trusted_str,
            &ep.submitted_at.to_string(),
        ]);
    }
    
    wtr.flush().expect("cannot flush csv");
    
    if let Err(e) = std::fs::rename(&tmp, path) {
        error!("save: rename {tmp} → {path} failed: {e}");
    }
}

// ============================================================================
// SHARED STATE
// ============================================================================

#[derive(Clone)]
struct AppState {
    endpoints: Arc<RwLock<Vec<Endpoint>>>,
}

// ============================================================================
// HELPERS
// ============================================================================

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn extract_domain(url: &str) -> Option<String> {
    let stripped = url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    Some(stripped.split('/').next()?.to_string())
}

// ============================================================================
// VALIDATOR — checks if /.well-known/mcp exists
// ============================================================================

async fn has_well_known(client: &reqwest::Client, url: &str) -> bool {
    let domain = match extract_domain(url) {
        Some(d) => d,
        None => return false,
    };

    let wk_url = format!("https://{}{}", domain, WELL_KNOWN_PATH);
    let timeout = Duration::from_secs(CHECK_TIMEOUT_S);

    match tokio::time::timeout(timeout, client.get(&wk_url).send()).await {
        Ok(Ok(resp)) => resp.status() != 404,
        _ => false,
    }
}

// ============================================================================
// MCP HANDLER (POST /mcp)
// ============================================================================

#[derive(Deserialize)]
struct RpcRequest {
    jsonrpc: String,
    id:      Option<Value>,
    method:  String,
    #[serde(default)]
    params:  Value,
}

#[derive(Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id:      Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result:  Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error:   Option<RpcError>,
}

#[derive(Serialize)]
struct RpcError { code: i32, message: String }

fn rpc_ok(id: Value, result: Value) -> axum::response::Response {
    Json(RpcResponse { jsonrpc: "2.0", id, result: Some(result), error: None }).into_response()
}

fn rpc_err(id: Value, code: i32, msg: impl Into<String>) -> axum::response::Response {
    Json(RpcResponse {
        jsonrpc: "2.0", id, result: None,
        error: Some(RpcError { code, message: msg.into() }),
    }).into_response()
}

fn text_result(id: Value, val: impl Serialize) -> axum::response::Response {
    rpc_ok(id, json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&val).unwrap_or_default() }]
    }))
}

async fn mcp_handler(
    State(state): State<AppState>,
    Json(req):    Json<RpcRequest>,
) -> axum::response::Response {

    if req.jsonrpc != "2.0" {
        return rpc_err(Value::Null, -32600, "jsonrpc must be \"2.0\"");
    }

    let id = req.id.clone().unwrap_or(Value::Null);

    match req.method.as_str() {

        "initialize" => rpc_ok(id, json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": { "name": "mcpub", "version": "0.1.0" },
            "capabilities": { "tools": {} }
        })),

        "notifications/initialized" => (StatusCode::NO_CONTENT, "").into_response(),

        "tools/list" => rpc_ok(id, json!({ "tools": [
            {
                "name": "submit",
                "description": "Register your MCP endpoint. First create an empty file at /.well-known/mcp on your domain. Then call this tool with your URL.",
                "inputSchema": {
                    "type": "object",
                    "required": ["url"],
                    "properties": {
                        "url":         { "type": "string", "description": "HTTPS base URL of your MCP server" },
                        "description": { "type": "string", "description": "What does your endpoint do?" }
                    }
                }
            },
            {
                "name": "search",
                "description": "Search live MCP endpoints by keyword. Only returns endpoints that are currently alive (have /.well-known/mcp).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Keyword to match against descriptions" },
                        "limit": { "type": "integer", "description": "Max results, default 10" }
                    }
                }
            },
            {
                "name": "list_all",
                "description": "List all endpoints (including dead ones — for debugging).",
                "inputSchema": { "type": "object", "properties": {} }
            }
        ]})),

        "tools/call" => {
            let tool = match req.params["name"].as_str() {
                Some(n) => n,
                None => return rpc_err(id, -32602, "params.name required"),
            };

            let args = &req.params["arguments"];

            match tool {

                // ── submit ────────────────────────────────────────────────────
                "submit" => {
                    let raw_url = match args["url"].as_str() {
                        Some(u) => u,
                        None => return rpc_err(id, -32602, "url is required"),
                    };

                    let url = match sanitize_url(raw_url) {
                        Ok(u) => u,
                        Err(e) => return rpc_err(id, -32602, e),
                    };

                    let raw_desc = args["description"].as_str().unwrap_or("");
                    let desc = sanitize_description(raw_desc);
                    
                    let client = reqwest::Client::new();

                    if !has_well_known(&client, &url).await {
                        return rpc_err(id, -32602, 
                            format!("No {} found on your domain. Create this file (can be empty) first.", WELL_KNOWN_PATH));
                    }

                    let mut eps = state.endpoints.write().await;

                    if eps.iter().any(|e| e.url == url) {
                        return rpc_err(id, -32602, "url already registered");
                    }

                    eps.push(Endpoint {
                        url: url.clone(),
                        description: desc,
                        trusted: false,
                        submitted_at: now_unix(),
                    });

                    save_csv(DATA_FILE, &eps);
                    info!("submit: {} (untrusted)", url);

                    text_result(id, json!({
                        "status": "registered",
                        "message": "Your endpoint is live. It will appear in search results immediately."
                    }))
                }

                // ── search ────────────────────────────────────────────────────
                "search" => {
                    let query = args["query"].as_str().unwrap_or("").to_lowercase();
                    let limit = args["limit"].as_u64().unwrap_or(10).min(50) as usize;
                    let client = reqwest::Client::new();
                    
                    let endpoints_copy: Vec<Endpoint> = {
                        let eps = state.endpoints.read().await;
                        eps.clone()
                    };
                    
                    let mut results = vec![];
                    let mut to_remove = vec![];
                    
                    for ep in endpoints_copy.iter() {
                        if ep.trusted {
                            if query.is_empty() || ep.description.to_lowercase().contains(&query) {
                                results.push(json!({
                                    "url": ep.url,
                                    "description": ep.description,
                                    "trusted": true
                                }));
                                if results.len() >= limit { break; }
                            }
                            continue;
                        }
                        
                        if has_well_known(&client, &ep.url).await {
                            if query.is_empty() || ep.description.to_lowercase().contains(&query) {
                                results.push(json!({
                                    "url": ep.url,
                                    "description": ep.description,
                                    "trusted": false
                                }));
                                if results.len() >= limit { break; }
                            }
                        } else {
                            to_remove.push(ep.url.clone());
                        }
                    }
                    
                    if !to_remove.is_empty() {
                        let mut eps_write = state.endpoints.write().await;
                        eps_write.retain(|e| !to_remove.contains(&e.url));
                        save_csv(DATA_FILE, &eps_write);
                        info!("search: removed {} dead user endpoints", to_remove.len());
                    }
                    
                    text_result(id, results)
                }

                // ── list_all ──────────────────────────────────────────────────
                "list_all" => {
                    let eps = state.endpoints.read().await;
                    let all: Vec<Value> = eps.iter()
                        .map(|e| json!({
                            "url": e.url,
                            "description": e.description,
                            "trusted": e.trusted,
                            "submitted_at": e.submitted_at
                        }))
                        .collect();
                    text_result(id, all)
                }

                other => rpc_err(id, -32601, format!("unknown tool: {other}")),
            }
        }

        other => {
            warn!("unknown method: {other}");
            rpc_err(id, -32601, format!("method not found: {other}"))
        }
    }
}

// ============================================================================
// MAIN
// ============================================================================

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_env("LOG_LEVEL")
                .add_directive("mcpub=info".parse().unwrap()),
        )
        .init();

    // Ensure data directory exists
    if let Some(parent) = std::path::Path::new(DATA_FILE).parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            error!("cannot create data dir {}: {}", parent.display(), e);
            std::process::exit(1);
        }
    }

    let endpoints = load_csv(DATA_FILE);
    info!("loaded {} endpoints from {}", endpoints.len(), DATA_FILE);

    let state = AppState {
        endpoints: Arc::new(RwLock::new(endpoints)),
    };

    let app = Router::new()
        .route("/mcp", post(mcp_handler))
        .with_state(state);

    info!("mcpub listening on {}", LISTEN_ADDR);
    info!("well-known path: {}", WELL_KNOWN_PATH);
    info!("data file: {}", DATA_FILE);

    let listener = TcpListener::bind(LISTEN_ADDR).await
        .unwrap_or_else(|e| panic!("cannot bind {}: {}", LISTEN_ADDR, e));

    axum::serve(listener, app).await.expect("server error");
}
