"""Offline safety checks; no production CSV or remote service is accessed."""

import contextlib
import csv
import io
import json
from pathlib import Path
import stat
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch
import urllib.error

import refresh_description as refresh


URL = "https://example.com/mcp"
FIELDS = ["url", "description", "trusted", "submitted_at", "extra"]
ROWS = [
    ["https://other.example/mcp", "Other, with a comma", "1", "123", "keep"],
    [URL, "Old description", "0", "456", "retain\nthis"],
]
MARKER = {"servers": [{"url": URL}]}
SOURCE = {"info": {"description": ' New "description"\nwith\tcontrols\x00 '}}
DESCRIPTION = "New 'description' with controls"


class ArchiveTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "endpoints.csv"
        with self.path.open("w", encoding="utf-8", newline="") as output:
            writer = csv.writer(output)
            writer.writerow(FIELDS)
            writer.writerows(ROWS)
        self.original = self.path.read_bytes()
        self.argv = ["refresh_description.py", "--archive", str(self.path),
                     "--url", URL, "--source-path", "/openapi.json",
                     "--description-pointer", "/info/description"]

    def run_cli(self, *arguments, metadata=None):
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(sys, "argv", self.argv + list(arguments)), \
                patch.object(refresh, "fetch_json", side_effect=metadata or [MARKER, SOURCE]), \
                contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            try:
                refresh.main()
                code = 0
            except SystemExit as error:
                code = error.code
        return code, stdout.getvalue(), stderr.getvalue()

    def assert_no_files_changed(self):
        self.assertEqual(self.path.read_bytes(), self.original)
        self.assertEqual(list(self.path.parent.iterdir()), [self.path])

    def test_default_dry_run_is_byte_for_byte_read_only(self):
        code, output, error = self.run_cli()
        self.assertEqual((code, error), (0, ""))
        receipt = json.loads(output)
        self.assertEqual(receipt["status"], "dry_run")
        self.assertEqual(receipt["records"], 2)
        self.assertEqual(receipt["old_description"], "Old description")
        self.assertEqual(receipt["description"], DESCRIPTION)
        self.assert_no_files_changed()

    def test_apply_preserves_all_other_values_and_backup(self):
        self.path.chmod(0o640)
        before = self.path.stat()
        code, output, error = self.run_cli("--apply", "--writers-stopped")
        self.assertEqual((code, error), (0, ""))
        receipt = json.loads(output)
        self.assertEqual(receipt["status"], "updated_on_disk")
        backup = Path(receipt["backup"])
        self.assertEqual(backup.read_bytes(), self.original)
        for path in [self.path, backup]:
            after = path.stat()
            self.assertEqual(stat.S_IMODE(after.st_mode), 0o640)
            self.assertEqual((after.st_uid, after.st_gid), (before.st_uid, before.st_gid))
        with self.path.open(encoding="utf-8", newline="") as source:
            actual = list(csv.reader(source))
        expected = [FIELDS] + [row[:] for row in ROWS]
        expected[2][1] = DESCRIPTION
        self.assertEqual(actual, expected)

    def test_repeated_apply_does_not_rewrite_or_create_another_backup(self):
        self.run_cli("--apply", "--writers-stopped")
        before = self.path.read_bytes(), self.path.stat().st_mtime_ns
        files = set(self.path.parent.iterdir())
        code, output, error = self.run_cli("--apply", "--writers-stopped")
        self.assertEqual((code, error), (0, ""))
        self.assertEqual(json.loads(output)["status"], "unchanged")
        self.assertEqual((self.path.read_bytes(), self.path.stat().st_mtime_ns), before)
        self.assertEqual(set(self.path.parent.iterdir()), files)

    def test_apply_requires_explicit_stopped_writers_assertion(self):
        code, output, error = self.run_cli("--apply")
        self.assertEqual((code, output), (2, ""))
        self.assertIn("--apply requires --writers-stopped", error)
        self.assert_no_files_changed()

    def test_unknown_or_duplicate_url_fails_without_network_or_mutation(self):
        for rows in [ROWS[:1], ROWS + [ROWS[1]]]:
            with self.subTest(rows=len(rows)):
                with self.path.open("w", newline="") as output:
                    writer = csv.writer(output)
                    writer.writerow(FIELDS)
                    writer.writerows(rows)
                self.original = self.path.read_bytes()
                with patch.object(refresh, "domain_description") as fetch:
                    code, output, error = self.run_cli("--apply", "--writers-stopped")
                    fetch.assert_not_called()
                self.assertEqual((code, output), (1, ""))
                self.assertIn("exactly one existing record", error)
                self.assert_no_files_changed()

    def test_invalid_csv_fails_without_mutation(self):
        cases = [
            "url,description,trusted\n",
            "description,url,trusted,submitted_at\n",
            "url,description,trusted,submitted_at,url\n",
            "url,description,trusted,submitted_at\n" + URL + ",old,0\n",
            "url,description,trusted,submitted_at\n" + URL + ",old,0,123,extra\n",
            "url,description,trusted,submitted_at\n" + URL + ',"unterminated,0,123\n',
        ]
        for content in cases:
            with self.subTest(content=content):
                self.path.write_text(content)
                self.original = self.path.read_bytes()
                code, output, _ = self.run_cli("--apply", "--writers-stopped")
                self.assertEqual((code, output), (1, ""))
                self.assert_no_files_changed()

    def test_symlink_is_rejected_even_in_dry_run(self):
        link = self.path.with_name("link.csv")
        link.symlink_to(self.path)
        with self.assertRaisesRegex(ValueError, "not a symlink"):
            refresh.existing_record(link, URL)
        self.assertEqual(self.path.read_bytes(), self.original)

    def test_metadata_failure_does_not_write_or_report_success(self):
        for metadata in [[{}], [MARKER, {}], [MARKER, {"info": {"description": " "}}]]:
            with self.subTest(metadata=metadata):
                code, output, _ = self.run_cli("--apply", "--writers-stopped", metadata=metadata)
                self.assertEqual((code, output), (1, ""))
                self.assert_no_files_changed()

    def test_intervening_file_change_is_not_overwritten(self):
        original, fields, rows, record = refresh.existing_record(self.path, URL)
        record["description"] = DESCRIPTION
        changed = self.original + b"https://new.example/mcp,new,0,789,new\r\n"
        self.path.write_bytes(changed)
        with self.assertRaisesRegex(ValueError, "archive changed"):
            refresh.replace_archive(self.path, original, fields, rows)
        self.assertEqual(self.path.read_bytes(), changed)
        self.assertEqual(list(self.path.parent.iterdir()), [self.path])

    def test_failed_replace_keeps_original_and_backup_and_reports_error(self):
        with patch.object(refresh.os, "replace", side_effect=OSError("replace failed")):
            code, output, error = self.run_cli("--apply", "--writers-stopped")
        self.assertEqual((code, output), (1, ""))
        self.assertIn("replace failed", error)
        self.assertEqual(self.path.read_bytes(), self.original)
        backups = list(self.path.parent.glob("endpoints.csv.bak-*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), self.original)
        self.assertEqual(set(self.path.parent.iterdir()), {self.path, backups[0]})

    def test_staging_failure_cleans_temporary_and_keeps_original(self):
        with patch.object(refresh.os, "fsync", side_effect=OSError("fsync failed")):
            code, output, error = self.run_cli("--apply", "--writers-stopped")
        self.assertEqual((code, output), (1, ""))
        self.assertIn("fsync failed", error)
        self.assert_no_files_changed()

    def test_backup_failure_prevents_replacement(self):
        with patch.object(refresh.time, "time_ns", return_value=123):
            backup = self.path.with_name("endpoints.csv.bak-123")
            backup.write_bytes(b"existing backup")
            code, output, _ = self.run_cli("--apply", "--writers-stopped")
        self.assertEqual((code, output), (1, ""))
        self.assertEqual(self.path.read_bytes(), self.original)
        self.assertEqual(backup.read_bytes(), b"existing backup")
        self.assertEqual(set(self.path.parent.iterdir()), {self.path, backup})


class MetadataTests(unittest.TestCase):
    def test_description_is_domain_sourced_and_sanitized(self):
        with patch.object(refresh, "fetch_json", side_effect=[MARKER, SOURCE]) as fetch:
            result = refresh.domain_description(URL, "/openapi.json", "/info/description")
        self.assertEqual(result, DESCRIPTION)
        self.assertEqual(fetch.call_args_list[0].args, ("https://example.com/.well-known/mcp.json",))
        self.assertEqual(fetch.call_args_list[1].args, ("https://example.com/openapi.json",))

    def test_marker_must_name_exact_registered_url(self):
        for marker in [{}, [], {"servers": {}}, {"servers": [{"url": URL + "/"}]}]:
            with self.subTest(marker=marker), patch.object(refresh, "fetch_json", return_value=marker) as fetch:
                with self.assertRaisesRegex(ValueError, "exact registered URL"):
                    refresh.domain_description(URL, "/openapi.json", "/info/description")
                self.assertEqual(fetch.call_count, 1)

    def test_invalid_url_or_cross_origin_source_is_rejected_before_fetch(self):
        cases = [("http://example.com/mcp", "/openapi.json"),
                 ("https://user:password@example.com/mcp", "/openapi.json"),
                 (URL + "?q=1", "/openapi.json"), (URL + "#part", "/openapi.json"),
                 (URL, "//other.example/openapi.json"), (URL, "https://other.example/meta")]
        for url, source in cases:
            with self.subTest(url=url, source=source), patch.object(refresh, "fetch_json") as fetch:
                with self.assertRaises(ValueError):
                    refresh.domain_description(url, source, "/info/description")
                fetch.assert_not_called()

    def test_empty_non_string_or_oversized_description_is_rejected(self):
        for value in ["", " \n\t\x00", 123, [], "x" * 8193]:
            with self.subTest(value_type=type(value).__name__), \
                    patch.object(refresh, "fetch_json", side_effect=[MARKER, {"info": {"description": value}}]):
                with self.assertRaises(ValueError):
                    refresh.domain_description(URL, "/openapi.json", "/info/description")

    def test_json_pointer_supports_escaped_keys_and_array_members(self):
        source = {"a/b": [{"~key": "A valid description"}]}
        with patch.object(refresh, "fetch_json", side_effect=[MARKER, source]):
            self.assertEqual(refresh.domain_description(URL, "/meta.json", "/a~1b/0/~0key"),
                             "A valid description")


class FetchTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path == "/redirect":
                    self.send_response(302)
                    self.send_header("Location", "/ok")
                    self.end_headers()
                    return
                self.send_response(500 if self.path == "/failure" else 200)
                self.end_headers()
                body = {"/ok": b'{"valid":true}', "/invalid": b"not JSON",
                        "/large": b"x" * (1024 * 1024 + 1)}.get(self.path, b"{}")
                self.wfile.write(body)

            def log_message(self, *_):
                pass

        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.origin = "http://127.0.0.1:" + str(cls.server.server_port)

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join()

    def test_valid_json(self):
        self.assertEqual(refresh.fetch_json(self.origin + "/ok"), {"valid": True})

    def test_redirect_is_not_followed(self):
        with self.assertRaises(urllib.error.HTTPError) as error:
            refresh.fetch_json(self.origin + "/redirect")
        self.assertEqual(error.exception.code, 302)

    def test_http_failure_is_rejected(self):
        with self.assertRaises(urllib.error.HTTPError) as error:
            refresh.fetch_json(self.origin + "/failure")
        self.assertEqual(error.exception.code, 500)

    def test_invalid_json_is_rejected(self):
        with self.assertRaises(ValueError):
            refresh.fetch_json(self.origin + "/invalid")

    def test_oversized_body_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "exceeds 1 MiB"):
            refresh.fetch_json(self.origin + "/large")


if __name__ == "__main__":
    unittest.main()
