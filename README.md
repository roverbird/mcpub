# 📡 mcpub
**Searchable open directory of remote MCP servers. No gatekeepers, no GitHub, no review required. Free open access!**

🌐 [mcpub.dev](https://mcpub.dev) | 🤖 `https://mcpub.dev/mcp`

_Model context protocol endpoint on every website._

---

## For agents

Connect to `https://mcpub.dev/mcp`. Tools: `submit`, `search`, `list_all`, `get`.

```json
{ "name": "search", "arguments": { "query": "crypto", "limit": 10, "offset": 0 } }
```

All list/search responses return `{ total, offset, limit, results }` — use `offset` to paginate.

---

## For humans

### Add your MCP server

1. Create `/.well-known/mcp.json` on your domain (any content, even empty)
2. Submit:

```bash
curl -X POST https://mcpub.dev/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"submit","arguments":{"url":"https://your.domain","description":"what it does"}}}'
```

### Search endpoints

```bash
curl -X POST https://mcpub.dev/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","arguments":{"query":"weather","limit":10,"offset":0}}}'
```

### Look up a specific endpoint

```bash
curl -X POST https://mcpub.dev/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get","arguments":{"url":"https://your.domain"}}}'
```

---

### Host your own MCP server for free

Turn any CLI into MCP with [suckless-mcp](https://github.com/roverbird/suckless-mcp):

```bash
curl -fsSL https://raw.githubusercontent.com/roverbird/suckless-mcp/main/install.sh | sh
```

---

[Contact](mailto:kibervarnost@proton.me)

---

**Just endpoints for all**
