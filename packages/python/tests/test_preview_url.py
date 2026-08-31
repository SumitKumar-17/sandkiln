"""Unit tests for `Sandbox.preview_url` — the one piece of genuinely pure,
network-free logic in this SDK (mirrors packages/sdk/test/previewUrl.test.js
on the JS side). Uses stdlib `unittest`, matching this package's
zero-added-dependency rule (see AGENTS.md).

Run: python packages/python/tests/test_preview_url.py
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from sandkiln import Sandbox  # noqa: E402


def sandbox(**kwargs) -> Sandbox:
    kwargs.setdefault("base_url", "http://127.0.0.1:7777")
    return Sandbox.attach("sbx-1", **kwargs)


class PreviewUrlTests(unittest.TestCase):
    def test_defaults_to_root_path_with_no_query_string(self) -> None:
        self.assertEqual(sandbox().preview_url(3000), "http://127.0.0.1:7777/sandboxes/sbx-1/preview/3000/")

    def test_adds_a_leading_slash_to_a_path_missing_one(self) -> None:
        self.assertEqual(
            sandbox().preview_url(3000, "api/health"),
            "http://127.0.0.1:7777/sandboxes/sbx-1/preview/3000/api/health",
        )

    def test_preserves_a_path_that_already_has_a_leading_slash(self) -> None:
        self.assertEqual(
            sandbox().preview_url(3000, "/api/health"),
            "http://127.0.0.1:7777/sandboxes/sbx-1/preview/3000/api/health",
        )

    def test_appends_the_auth_token_as_a_query_parameter_when_configured(self) -> None:
        self.assertEqual(
            sandbox(auth_token="secret123").preview_url(3000),
            "http://127.0.0.1:7777/sandboxes/sbx-1/preview/3000/?token=secret123",
        )

    def test_combines_a_custom_path_with_the_auth_token_query_parameter(self) -> None:
        self.assertEqual(
            sandbox(auth_token="secret123").preview_url(3000, "/app"),
            "http://127.0.0.1:7777/sandboxes/sbx-1/preview/3000/app?token=secret123",
        )

    def test_rejects_an_out_of_range_or_non_integer_port(self) -> None:
        for bad_port in (0, 65536, 3.5, -1, True):
            with self.assertRaises(ValueError):
                sandbox().preview_url(bad_port)


if __name__ == "__main__":
    unittest.main()
