# 📡 mcpub

**Searchable open directory of remote MCP servers. No gatekeepers, no GitHub, no review required. Free open access!**

🌐 [mcpub.dev](https://mcpub.dev) | 🤖 `https://mcpub.dev/mcp`

_Model context protocol endpoint on every website._
 
---

## For agents

Connect to `https://mcpub.dev/mcp`. Tools: `submit`, `search`, `list_all`.

```json
{
  "name": "search",
  "arguments": { "query": "crypto", "limit": 10 }
}
```

---

## For humans

### Add your MCP server

0. Any remote servers accepted!
1. Create `/.well-known/mcp.json` on your domain (any content allowed, even empty)
2. Submit:
```bash
curl -X POST https://mcpub.dev/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"submit","arguments":{"url":"https://your.domain","description":"what it does"}}}'
```

### Search endpoints
```bash
curl -X POST https://mcpub.dev/mcp \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","arguments":{"query":"weather"}}}'
```

### Host your own MCP server

Turn any CLI into MCP with [suckless-mcp](https://github.com/roverbird/suckless-mcp):
```bash
curl -fsSL https://raw.githubusercontent.com/roverbird/suckless-mcp/main/install.sh | sh
```

---

[Contacts](mailto:kibervarnost@proton.me)

---

**Just endpoints for all**

