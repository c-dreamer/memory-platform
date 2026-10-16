import tempfile
import unittest
from pathlib import Path

from dashboard_core import dashboard_url, read_token


class DashboardCoreTests(unittest.TestCase):
    def test_reads_and_trims_token(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "dashboard.token"
            path.write_text("a" * 40 + "\n", encoding="utf-8")
            self.assertEqual(read_token(path), "a" * 40)

    def test_rejects_short_token(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "dashboard.token"
            path.write_text("short\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "too short"):
                read_token(path)

    def test_builds_loopback_operations_url_and_encodes_token(self):
        token = "abc/def+ghi" + "x" * 24
        url = dashboard_url(token)
        self.assertTrue(url.startswith("http://127.0.0.1:8765/operations?token="))
        self.assertIn("%2F", url)
        self.assertIn("%2B", url)
        self.assertNotIn("&", url)

    def test_rejects_short_url_token(self):
        with self.assertRaisesRegex(ValueError, "too short"):
            dashboard_url("short")


if __name__ == "__main__":
    unittest.main()
