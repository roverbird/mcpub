# 📡 mcpub

**Searchable open directory of remote MCP servers, which is itself and open MCP**  
No gatekeepers. No GitHub required. No review. Free open access.

> 🚀 [`mcpub.dev`](https://mcpub.dev) · 🤖 `https://mcpub.dev/mcp`

---

## ✨ What is this?

Every website can have an MCP endpoint.  
This is the directory that finds them all.

- **For agents** — search, list, and discover live MCP servers programmatically  
- **For humans** — submit your server, search the directory, or host your own MCP endpoint for free

---

## 🤖 For agents

**Connect to:** `https://mcpub.dev/mcp`

### Tools

| Tool | Description |
|------|-------------|
| `submit` | Add a new MCP endpoint |
| `search` | Search archived endpoints |
| `list_all` | List all archived endpoints (paginated) |
| `get` | Look up a specific endpoint by URL |
| `search_live` | Search only verified *alive* endpoints |
| `list_all_live` | List only verified *alive* endpoints |

### Example: search

```json
{
  "name": "search",
  "arguments": {
    "query": "crypto",
    "limit": 10,
    "offset": 0
  }
}
```

All `list`/`search` responses return:
```json
{
  "total": 42,
  "offset": 0,
  "limit": 10,
  "results": [...]
}
```
Use `offset` to paginate.

### Live tools

`search_live` and `list_all_live` return **only verified alive endpoints** — automatically updated by [`mcp-spider`](https://github.com/roverbird/mcp-spider).

---

## 👤 For humans

### Add your MCP server to the list

1. Create `/.well-known/mcp.json` on your domain (any content works — even empty `{}`)
2. Submit:

```bash
curl -X POST https://mcpub.dev/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {
      "name": "submit",
      "arguments": {
        "url": "https://your.domain",
        "description": "what it does"
      }
    }
  }'
```

### Search endpoints

```bash
curl -X POST https://mcpub.dev/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {
      "name": "search",
      "arguments": {
        "query": "weather",
        "limit": 10,
        "offset": 0
      }
    }
  }'
```

### Search only live endpoints

```bash
curl -X POST https://mcpub.dev/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {
      "name": "search_live",
      "arguments": {
        "query": "satellite",
        "limit": 10
      }
    }
  }'
```

### Look up a specific endpoint

```bash
curl -X POST https://mcpub.dev/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {
      "name": "get",
      "arguments": {
        "url": "https://your.domain"
      }
    }
  }'
```

---

## ⚡ Host your own MCP server (for free)

Turn any CLI tool into a compliant MCP endpoint with [`suckless-mcp`](https://github.com/roverbird/suckless-mcp):

```bash
curl -fsSL https://raw.githubusercontent.com/roverbird/suckless-mcp/main/install.sh | sh
```

- One Rust binary  
- One directory of skills  
- No framework, no bullshit

---

## 🕷️ Keep the directory clean

[`mcp-spider`](https://github.com/roverbird/mcp-spider) periodically scans all endpoints and maintains the `search_live` cache with **verified alive servers**.

Dead endpoints stay in the archive (for history) but won't appear in live results.

---

## 📬 Contact

Just endpoints for all.  
Open source. Open data. Open protocol.

**[mcpub.dev](https://mcpub.dev)** · [GitHub](https://github.com/roverbird/mcpub)

---

*Model Context Protocol endpoint on every website, for free*

