"""Regression tests for profile-isolated compatibility fixture discovery."""

from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

from compat_fixtures import discover_fixtures


ROOT = Path(__file__).resolve().parents[1] / "tests" / "fixtures"


class FixtureDiscoveryTests(unittest.TestCase):
    def test_checked_in_manifest_oracles_are_explicit_and_generic_are_separate(self):
        legacy = {
            path.relative_to(ROOT).as_posix()
            for path in discover_fixtures(ROOT, "legacy_manifest_oracle")
        }
        generic = {
            path.relative_to(ROOT).as_posix()
            for path in discover_fixtures(ROOT, "generic")
        }
        self.assertEqual(legacy, {"fork-semantic", "library", "menagerie", "transport"})
        self.assertIn("upstream-semantic", generic)
        self.assertIn("upstream-structural", generic)
        self.assertFalse(legacy & generic)
        self.assertEqual(
            legacy | generic,
            {p.relative_to(ROOT).as_posix() for p in discover_fixtures(ROOT, "all")},
        )

    def test_new_nested_generic_never_enters_legacy_oracle(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "compatibility-cases.json").write_text(
                json.dumps({"schema_version": 1, "legacy_manifest_oracle": ["oracle"]})
            )
            oracle = root / "oracle"
            oracle.mkdir()
            (oracle / "openapi.json").write_text("{}")
            (oracle / "binding-manifest.json").write_text("{}")
            generic = root / "capability-v1" / "json"
            generic.mkdir(parents=True)
            (generic / "openapi.json").write_text("{}")
            raw = root / "other" / "raw"
            raw.mkdir(parents=True)
            (raw / "openapi.json").write_text("{}")
            self.assertEqual(discover_fixtures(root, "legacy_manifest_oracle"), [oracle])
            self.assertEqual(discover_fixtures(root, "generic"), [generic])

    def test_misclassified_or_missing_legacy_fails_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "compatibility-cases.json").write_text(
                json.dumps({"schema_version": 1, "legacy_manifest_oracle": ["oracle"]})
            )
            oracle = root / "oracle"
            oracle.mkdir()
            (oracle / "openapi.json").write_text("{}")
            with self.assertRaisesRegex(ValueError, "lacks manifest evidence"):
                discover_fixtures(root, "legacy_manifest_oracle")
            (oracle / "openapi.json").unlink()
            with self.assertRaisesRegex(ValueError, "missing openapi.json"):
                discover_fixtures(root, "generic")


if __name__ == "__main__":
    unittest.main()
