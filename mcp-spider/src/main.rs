// mcp-spider: scan MCP endpoints for liveness and tool enumeration
// Usage: mcp-spider -i endpoints.csv -o scan_cache.csv --deep

use clap::Parser;
use csv::ReaderBuilder;
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

// ---------------------------------------------------------------------------
// CLI - optimized defaults for max discovery
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "mcp-spider", about = "Probe MCP endpoints for liveness and tools")]
struct Cli {
    #[arg(short, long, default_value = "endpoints.csv")]
    input: PathBuf,

    #[arg(short, long)]
    output: Option<PathBuf>,

    #[arg(long)]
    deep: bool,

    #[arg(long)]
    skip_dead: bool,

    #[arg(long)]
    keep_dead: bool,

    #[arg(short, long, default_value = "30")]
    concurrency: usize,

    #[arg(long, default_value = "15")]
    timeout: u64,

    #[arg(long, default_value = "2")]
    retries: u32,
}

// ---------------------------------------------------------------------------
// Input record
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
struct InputRecord {
    url: String,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

impl InputRecord {
    fn is_marked_dead(&self) -> bool {
        self.extra
            .get("alive")
            .and_then(|v| v.as_str())
            .map(|v| v == "0")
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Output record for mcpub
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct McpubRecord {
    url: String,
    description: String,
    trusted: String,
    submitted_at: String,
}

// ---------------------------------------------------------------------------
// MCP JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct RpcRequest<P: Serialize> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: P,
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const PATH_CANDIDATES: &[&str] = &["/mcp", "", "/sse", "/api/mcp", "/v1/mcp"];

// ---------------------------------------------------------------------------
// Well-known discovery
// ---------------------------------------------------------------------------

async fn discover_well_known(client: &Client, base_url: &str, timeout_secs: u64) -> Option<String> {
    let origin = {
        let stripped = base_url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let scheme = if base_url.starts_with("http://") { "http" } else { "https" };
        let host = stripped.split('/').next()?;
        format!("{scheme}://{host}")
    };

    let well_known_url = format!("{origin}/.well-known/mcp.json");

    let resp = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        client.get(&well_known_url).send(),
    )
    .await
    .ok()?
    .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let json: Value = resp.json().await.ok()?;

    json.pointer("/server/endpoints/streamable-http")
        .or_else(|| json.pointer("/endpoints/streamable-http"))
        .or_else(|| json.pointer("/url"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Retry helper
// ---------------------------------------------------------------------------

async fn with_retry<F, Fut, T>(mut f: F, max_attempts: u32) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut last_err = String::from("no attempts made");
    for attempt in 0..max_attempts.max(1) {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_err = e;
                if attempt + 1 < max_attempts {
                    tokio::time::sleep(Duration::from_millis(100 * 2_u64.pow(attempt))).await;
                }
            }
        }
    }
    Err(last_err)
}

// ---------------------------------------------------------------------------
// Probe result
// ---------------------------------------------------------------------------

struct ProbeResult {
    url: String,
    alive: bool,
    server_name: String,
    tools: Option<Vec<String>>,
    error: Option<String>,
    latency_ms: u64,
    protocol_version: String,
    session_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Top-level probe
// ---------------------------------------------------------------------------

async fn probe(
    client: &Client,
    base_url: &str,
    deep: bool,
    timeout_secs: u64,
    retries: u32,
) -> ProbeResult {
    let base = base_url.trim_end_matches('/');
    let start = Instant::now();

    let well_known = discover_well_known(client, base_url, timeout_secs).await;

    let mut candidates: Vec<String> = Vec::new();
    if let Some(ref wk_url) = well_known {
        candidates.push(wk_url.clone());
    }
    for &suffix in PATH_CANDIDATES {
        let url = format!("{base}{suffix}");
        if !candidates.contains(&url) {
            candidates.push(url);
        }
    }

    let mut last_error = String::from("no endpoint found");

    for url in &candidates {
        let result = with_retry(
            || async { try_initialize(client, url, timeout_secs).await },
            retries,
        )
        .await;

        match result {
            Ok(init_ok) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                let tools = if deep {
                    fetch_tools(client, url, init_ok.session_id.as_deref(), timeout_secs).await
                } else {
                    None
                };

                return ProbeResult {
                    url: base_url.to_string(),
                    alive: true,
                    server_name: init_ok.server_name,
                    tools,
                    error: None,
                    latency_ms,
                    protocol_version: init_ok.protocol_version,
                    session_id: init_ok.session_id,
                };
            }
            Err(e) => {
                last_error = e;
            }
        }
    }

    ProbeResult {
        url: base_url.to_string(),
        alive: false,
        server_name: String::new(),
        tools: None,
        error: Some(last_error),
        latency_ms: start.elapsed().as_millis() as u64,
        protocol_version: String::new(),
        session_id: None,
    }
}

// ---------------------------------------------------------------------------
// MCP initialize
// ---------------------------------------------------------------------------

struct InitOk {
    protocol_version: String,
    server_name: String,
    session_id: Option<String>,
}

async fn try_initialize(client: &Client, url: &str, timeout_secs: u64) -> Result<InitOk, String> {
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "mcp-spider", "version": "0.2" }
        }),
    };

    let resp = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        client.post(url).json(&body).send(),
    )
    .await
    .map_err(|_| "timeout".to_string())?
    .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let rpc: RpcResponse = resp.json().await.map_err(|e| format!("JSON: {e}"))?;

    if let Some(err) = rpc.error {
        return Err(format!("RPC {}: {}", err.code, err.message));
    }

    let result = rpc.result.ok_or("missing result")?;

    let protocol_version = result
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .ok_or("missing protocolVersion")?
        .to_string();

    let server_name = result
        .pointer("/serverInfo/name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(InitOk { protocol_version, server_name, session_id })
}

// ---------------------------------------------------------------------------
// tools/list
// ---------------------------------------------------------------------------

async fn fetch_tools(
    client: &Client,
    url: &str,
    session_id: Option<&str>,
    timeout_secs: u64,
) -> Option<Vec<String>> {
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 2,
        method: "tools/list",
        params: json!({}),
    };

    let mut req = client.post(url).json(&body);
    if let Some(sid) = session_id {
        req = req.header("mcp-session-id", sid);
    }

    let resp = tokio::time::timeout(Duration::from_secs(timeout_secs), req.send())
        .await
        .ok()?
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let rpc: RpcResponse = resp.json().await.ok()?;
    let result = rpc.result?;
    let tools_arr = result.get("tools")?.as_array()?;

    Some(
        tools_arr
            .iter()
            .filter_map(|t| t.get("name")?.as_str().map(str::to_string))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Build description
// ---------------------------------------------------------------------------

fn build_description(original_desc: &str, probe: &ProbeResult) -> String {
    let mut parts = Vec::new();
    
    if !original_desc.is_empty() {
        parts.push(original_desc.to_string());
    }
    
    let mut stats = Vec::new();
    if !probe.server_name.is_empty() {
        stats.push(format!("server: {}", probe.server_name));
    }
    stats.push(format!("latency: {}ms", probe.latency_ms));
    stats.push(format!("protocol: {}", probe.protocol_version));
    
    if let Some(tools) = &probe.tools {
        if !tools.is_empty() {
            stats.push(format!("tools: {}", tools.len()));
            let preview: Vec<&str> = tools.iter().take(6).map(|s| s.as_str()).collect();
            stats.push(format!("first: {}", preview.join(", ")));
        }
    }
    
    if !stats.is_empty() {
        parts.push(format!("[{}]", stats.join(" | ")));
    }
    
    if parts.is_empty() {
        "MCP server".to_string()
    } else {
        parts.join(" ")
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Read input
    let mut rdr = ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_path(&cli.input)?;

    let all_records: Vec<InputRecord> = rdr.deserialize().collect::<Result<_, _>>()?;
    let total_input = all_records.len();

    // Filter if skip_dead
    let records: Vec<InputRecord> = if cli.skip_dead {
        let filtered: Vec<_> = all_records.iter().filter(|r| !r.is_marked_dead()).cloned().collect();
        eprintln!("Loaded {} endpoints ({} skipped dead)", filtered.len(), total_input - filtered.len());
        filtered
    } else {
        eprintln!("Loaded {} endpoints", total_input);
        all_records.clone()
    };

    // HTTP client
    let client = Arc::new(Client::builder()
        .timeout(Duration::from_secs(cli.timeout + 2))
        .user_agent("mcp-spider/0.2")
        .default_headers({
            let mut h = header::HeaderMap::new();
            h.insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json"),
            );
            h
        })
        .build()?);

    let sem = Arc::new(Semaphore::new(cli.concurrency));
    let mut handles = Vec::new();
    let mut record_map = Vec::new();

    for rec in records {
        let permit = sem.clone().acquire_owned().await?;
        let client = client.clone();
        let url = rec.url.clone();
        let deep = cli.deep;
        let timeout = cli.timeout;
        let retries = cli.retries;

        record_map.push(rec);
        handles.push(tokio::spawn(async move {
            let _permit = permit;
            probe(&client, &url, deep, timeout, retries).await
        }));
    }

    // Collect results
    let mut mcpub_records = Vec::new();
    let mut alive_count = 0;

    for (input_rec, handle) in record_map.into_iter().zip(handles.into_iter()) {
        let probe_result = match handle.await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Task failed for {}: {}", input_rec.url, e);
                continue;
            }
        };

        if probe_result.alive {
            alive_count += 1;
            let original_desc = input_rec
                .extra
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            
            mcpub_records.push(McpubRecord {
                url: probe_result.url.clone(),  // ← FIX: added .clone()
                description: build_description(&original_desc, &probe_result),
                trusted: "1".to_string(),
                submitted_at: "0".to_string(),
            });
        } else if cli.keep_dead {
            let original_desc = input_rec
                .extra
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            
            let desc = if !original_desc.is_empty() {
                format!("[DEAD] {}", original_desc)
            } else {
                format!("[DEAD] {}", probe_result.error.unwrap_or_default())
            };
            
            mcpub_records.push(McpubRecord {
                url: probe_result.url.clone(),  // ← FIX: added .clone()
                description: desc,
                trusted: "0".to_string(),
                submitted_at: "0".to_string(),
            });
        }
    }

    eprintln!("✓ {}/{} alive", alive_count, total_input);

    // Write output - headers handled automatically by serialize
    if let Some(out_path) = cli.output {
        let mut wtr = csv::Writer::from_path(&out_path)?;
        for rec in &mcpub_records {
            wtr.serialize(rec)?;
        }
        wtr.flush()?;
        eprintln!("Written {} endpoints → {:?}", mcpub_records.len(), out_path);
    } else {
        for rec in &mcpub_records {
            println!("{}", serde_json::to_string(rec)?);
        }
    }

    Ok(())
}
