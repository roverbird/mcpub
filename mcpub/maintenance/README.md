# Correct an existing description without a duplicate record

This is an optional maintainer-only maintenance utility. It adds no HTTP route or
public MCP tool, and leaves `submit` and its duplicate-URL rejection unchanged.
Write access to the host CSV is the authorization boundary; the public
`/.well-known/mcp.json` marker is not caller authentication.

The utility fetches the marker over HTTPS, requires HTTP 2xx and a `servers[]`
entry naming the exact existing URL, then retrieves a string from an explicitly
chosen same-origin JSON source. Redirects are rejected and response bodies are
bounded. A description is never taken from an unverified caller's request.
Empty/malformed metadata fails without changing the CSV. Python 3.8+ on Linux is
required. Run under the CSV owner or an authorized maintainer account.

For an existing record whose manifest carries a description, preview it:

```sh
python3 mcpub/maintenance/refresh_description.py \
  --archive /var/lib/mcpub/endpoints.csv \
  --url https://example.com/mcp \
  --source-path /.well-known/mcp.json \
  --description-pointer /servers/0/description
```

Choose the JSON pointer for the intended entry. An alternative same-origin JSON
source is supported: for issue [#5](https://github.com/roverbird/mcpub/issues/5),
the Speedbot proposal uses `--url https://speedbot.dev/mcp`,
`--source-path /openapi.json` and `--description-pointer /info/description`.
The well-known manifest must still name the exact registered URL.

Dry run is the default. Review the source and preview. Before applying, stop the
actual mcpub serving process **and all other writers of that CSV** through the
deployment's service manager. This utility does not detect or stop services;
`--writers-stopped` is an explicit operator assertion, not a process lock.

```sh
python3 mcpub/maintenance/refresh_description.py \
  --archive /var/lib/mcpub/endpoints.csv \
  --url https://example.com/mcp \
  --source-path /.well-known/mcp.json \
  --description-pointer /servers/0/description \
  --apply --writers-stopped
```

Exactly one existing row must match. Only its description changes; registration
time, trust, URL, row count, other row values and extra CSV fields remain intact.
The first four columns must be `url,description,trusted,submitted_at`, matching
the server's positional CSV reader. CSV quoting and line endings may be rewritten.
The tool creates a backup, detects an intervening file change, stages a complete
CSV on the same filesystem and atomically replaces it. Repeated application of
the same description reports `unchanged` and creates no new row or backup.
Coordinate all writers: the changed-file check cannot prevent a race with an
unpaused process. I/O failures are errors, not a successful refresh receipt.

mcpub loads the archive and live cache at startup. A disk edit alone is not a
public update and can be overwritten by its old in-memory archive. Restart or
reload the serving process after the correction, then verify public `get` and
archive `search`. Keep the backup until verification succeeds.

If this exact URL already occurs once in `scan_cache.csv`, pause its scanner
writer too and apply the same procedure to that file before restarting mcpub.
If it is absent, leave it absent. Do not insert a row or turn a metadata refresh
into a liveness claim. The spider reuses the archive description, so rescanning
alone cannot correct stale archive text.

This does not claim a general owner-edit API. A later public `refresh(url)` tool
could read a URL-bound description directly from the well-known manifest, but
must not accept arbitrary caller-supplied text after the existence-only marker
check.

## Reproduce the safety checks

From the repository root, with no dependencies beyond the Python standard library:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s mcpub/maintenance -p 'test_*.py' -v
```

The tests use temporary CSVs, fixture metadata and a loopback HTTP server. They
do not contact public endpoints or read production data. The dry-run test verifies
that CSV bytes and directory contents stay unchanged. Apply tests verify a
one-description update, unchanged metadata and other rows, backup contents and
permissions, idempotence, and refusal of unknown or duplicate URLs. Failure tests
cover malformed CSV/metadata, redirects, oversized bodies, changed files and
failed persistence. `--writers-stopped` remains an operator assertion; these tests
do not establish coordination with real running services.
