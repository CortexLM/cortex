#!/usr/bin/env python3
"""rewrite_network.py: agent path is public; compose isolation is dropped."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "harness"))
import rewrite_network  # noqa: E402


class RewriteNetworkTests(unittest.TestCase):
    def test_toml_no_network_becomes_public(self) -> None:
        src = '[environment]\nnetwork_mode = "no-network"\n[agent]\nnetwork_mode = "none"\n'
        out = rewrite_network.rewrite_toml(src, "public")
        self.assertIn('network_mode = "public"', out)
        self.assertNotIn("no-network", out)
        self.assertNotIn("none", out)

    def test_yaml_network_mode_none_dropped(self) -> None:
        src = "services:\n  main:\n    network_mode: none\n    image: demo\n"
        out = rewrite_network.rewrite_yaml(src)
        self.assertNotIn("network_mode", out)
        self.assertIn("image: demo", out)

    def test_tree_rewrite(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            task = root / "quick"
            task.mkdir()
            (task / "task.toml").write_text(
                '[environment]\nnetwork_mode = "no-network"\n',
                encoding="utf-8",
            )
            (task / "docker-compose.yml").write_text(
                "services:\n  main:\n    network_mode: none\n",
                encoding="utf-8",
            )
            stats = rewrite_network.rewrite_tree(root, "public")
            self.assertEqual(stats["toml"], 1)
            self.assertEqual(stats["yaml"], 1)
            self.assertEqual(stats.get("json", 0), 0)
            toml = (task / "task.toml").read_text(encoding="utf-8")
            yaml = (task / "docker-compose.yml").read_text(encoding="utf-8")
            self.assertIn('network_mode = "public"', toml)
            self.assertNotIn("network_mode", yaml)

    def test_json_no_network_becomes_public(self) -> None:
        src = '{"network_mode": "no-network", "other": 1}'
        out = rewrite_network.rewrite_json(src, "public")
        self.assertIn('"network_mode": "public"', out)
        self.assertNotIn("no-network", out)

    def test_refuses_non_public_mode(self) -> None:
        with self.assertRaises(SystemExit):
            rewrite_network.main(["--tasks-dir", ".", "--mode", "no-network"])


if __name__ == "__main__":
    unittest.main()
