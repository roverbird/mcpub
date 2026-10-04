#!/usr/bin/env python3
"""Maintainer-only correction of one existing mcpub CSV description."""

import argparse
import csv
import io
import json
import os
from pathlib import Path
import stat
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


class NoRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def fetch_json(url):
    request = urllib.request.Request(url, headers={"Accept": "application/json"})
    with urllib.request.build_opener(NoRedirects).open(request, timeout=5) as response:
        if not 200 <= response.status < 300:
            raise ValueError("metadata must return HTTP 2xx")
        body = response.read(1024 * 1024 + 1)
        if len(body) > 1024 * 1024:
            raise ValueError("metadata exceeds 1 MiB")
        return json.loads(body)


def domain_description(url, source_path, pointer):
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != "https" or not parsed.hostname or parsed.username is not None
            or parsed.password is not None or parsed.query or parsed.fragment):
        raise ValueError("use the exact registered HTTPS URL without credentials/query/fragment")
    if not source_path.startswith("/") or source_path.startswith("//"):
        raise ValueError("source path must be an absolute path on the same origin")
    origin = "https://" + parsed.netloc
    source_url = urllib.parse.urljoin(origin + "/", source_path)
    if urllib.parse.urlsplit(source_url).netloc != parsed.netloc:
        raise ValueError("metadata source must stay on the registered origin")
    marker = fetch_json(origin + "/.well-known/mcp.json")
    servers = marker.get("servers", []) if isinstance(marker, dict) else []
    if not isinstance(servers, list) or not any(
        isinstance(server, dict) and server.get("url") == url for server in servers
    ):
        raise ValueError("well-known marker must explicitly name the exact registered URL")
    if not pointer.startswith("/"):
        raise ValueError("description pointer must be a JSON pointer")
    value = fetch_json(source_url)
    for token in pointer[1:].split("/"):
        token = token.replace("~1", "/").replace("~0", "~")
        value = value[int(token)] if isinstance(value, list) else value[token]
    if not isinstance(value, str):
        raise ValueError("description must be a string")
    description = "".join(c for c in value if not (
        ord(c) <= 8 or 11 <= ord(c) <= 12 or 14 <= ord(c) <= 31 or ord(c) == 127
    )).replace('"', "'").replace("\n", " ").replace("\r", " ").replace("\t", " ").strip()
    if not description or len(description) > 8192:
        raise ValueError("description must be nonempty and at most 8192 characters")
    return description


def existing_record(path, url):
    if path.is_symlink() or not path.is_file():
        raise ValueError("archive must be a regular file, not a symlink")
    original = path.read_bytes()
    reader = csv.DictReader(io.StringIO(original.decode("utf-8"), newline=""), strict=True)
    fields = reader.fieldnames
    if (fields is None or len(set(fields)) != len(fields)
            or fields[:4] != ["url", "description", "trusted", "submitted_at"]):
        raise ValueError("archive requires url,description,trusted,submitted_at as its first columns")
    rows = list(reader)
    if any(None in row or any(v is None for v in row.values()) for row in rows):
        raise ValueError("malformed CSV; refusing to rewrite")
    matches = [row for row in rows if row["url"] == url]
    if len(matches) != 1:
        raise ValueError("expected exactly one existing record; will not insert or deduplicate")
    return original, fields, rows, matches[0]


def replace_archive(path, original, fields, rows):
    if path.is_symlink() or not path.is_file():
        raise ValueError("archive must be a regular file, not a symlink")
    original_stat = path.stat()
    mode = stat.S_IMODE(original_stat.st_mode)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", newline="",
                                         dir=path.parent, delete=False) as output:
            temporary = Path(output.name)
            writer = csv.DictWriter(output, fieldnames=fields)
            writer.writeheader()
            writer.writerows(rows)
            output.flush()
            os.fsync(output.fileno())
        temporary_stat = temporary.stat()
        if (temporary_stat.st_uid, temporary_stat.st_gid) != (original_stat.st_uid, original_stat.st_gid):
            os.chown(temporary, original_stat.st_uid, original_stat.st_gid)
        os.chmod(temporary, mode)
        if path.read_bytes() != original:
            raise ValueError("archive changed during preparation; retry with all writers stopped")
        backup = path.with_name(path.name + ".bak-" + str(time.time_ns()))
        with backup.open("xb") as output:
            output.write(original)
            output.flush()
            os.fsync(output.fileno())
        backup_stat = backup.stat()
        if (backup_stat.st_uid, backup_stat.st_gid) != (original_stat.st_uid, original_stat.st_gid):
            os.chown(backup, original_stat.st_uid, original_stat.st_gid)
        os.chmod(backup, mode)
        os.replace(temporary, path)
        temporary = None
        directory_fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
        return backup
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--url", required=True)
    parser.add_argument("--source-path", required=True)
    parser.add_argument("--description-pointer", required=True)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--writers-stopped", action="store_true",
                        help="operator assertion: mcpub and every writer of this CSV are stopped")
    args = parser.parse_args()
    if args.apply and not args.writers_stopped:
        parser.error("--apply requires --writers-stopped; this tool cannot stop services for you")
    try:
        original, fields, rows, record = existing_record(args.archive, args.url)
        description = domain_description(args.url, args.source_path, args.description_pointer)
        if record["description"] == description:
            print(json.dumps({"status": "unchanged", "url": args.url, "records": len(rows)}))
            return
        preview = {"status": "dry_run", "url": args.url, "records": len(rows),
                   "old_description": record["description"], "description": description}
        if args.apply:
            record["description"] = description
            backup = replace_archive(args.archive, original, fields, rows)
            preview.update(status="updated_on_disk", backup=str(backup),
                           next_step="restart/reload mcpub and verify public get/search")
        print(json.dumps(preview, ensure_ascii=False))
    except (OSError, ValueError, LookupError, TypeError, csv.Error, urllib.error.URLError) as error:
        parser.exit(1, str(error) + "\n")


if __name__ == "__main__":
    main()
