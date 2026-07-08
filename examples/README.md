# Submission Examples

This folder contains ready-to-edit JSON-RPC payloads for the `submit` tool.

Use them to verify that a listing has:

- an HTTPS base URL, without query strings or fragments
- a concise description with searchable terms
- a reachable `/.well-known/mcp.json` marker on the submitted domain

Run a syntax check before copying a payload into `curl`:

```bash
python3 -m json.tool examples/xquik-submit.json >/dev/null
```

After submitting, call `get` with the same URL to confirm the normalized entry.
