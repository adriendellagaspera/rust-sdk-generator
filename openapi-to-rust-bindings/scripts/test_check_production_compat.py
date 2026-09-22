"""Production compatibility classification and tracker regressions."""

from __future__ import annotations

from pathlib import Path
import unittest

import check_production_compat as compat


class ProductionCompatibilityTests(unittest.TestCase):
    def test_checked_in_tracker_matches_default_and_supported_envelope(self):
        root = Path(__file__).resolve().parents[2]
        tracker = compat.load_tracker(
            root, root / "openapi-to-rust-bindings/COMPATIBILITY.json"
        )
        self.assertEqual(tracker["schema_version"], 3)
        self.assertEqual(tracker["backend"]["repository"], "gpu-cli/openapi-to-rust")
        self.assertFalse(tracker["boundary"]["producer_manifest_required"])

    def test_source_coverage_is_exact_and_reports_unemitted_operations(self):
        spec = {
            "paths": {
                "/a": {"get": {"operationId": "read_a"}},
                "/b": {"post": {"operationId": "write_b"}},
            }
        }
        bindings = {
            "operations": {
                "read_a": {
                    "metadata": {
                        "source_operation": {
                            "operation_id": "read_a",
                            "method": "GET",
                            "path": "/a",
                        }
                    }
                }
            }
        }
        with self.assertRaisesRegex(ValueError, "write_b"):
            compat.assert_source_coverage(spec, bindings)

    def test_capability_observation_keeps_failure_ownership_distinct(self):
        bindings = {
            "operations": {
                "download_blob": {
                    "metadata": {
                        "source_operation": {
                            "operation_id": "download_blob",
                            "method": "GET",
                            "path": "/blob",
                        },
                        "representation": {"kind": "binary_buffered"},
                        "kind": "call_shape",
                        "request_discriminators": [],
                    }
                }
            }
        }
        supported = {
            "id": "buffered_binary",
            "operation_id": "download_blob",
            "selector": {"representation": "binary_buffered"},
        }
        absent = {
            "id": "binary_stream",
            "operation_id": "download_blob",
            "selector": {"representation": "binary_stream"},
        }
        self.assertEqual(
            compat.capability_observation(bindings, None, supported)["status"],
            "supported",
        )
        raw_gap = compat.capability_observation(bindings, None, absent)
        self.assertEqual(raw_gap["status"], "raw_generation_gap")
        self.assertEqual(raw_gap["diagnostic"], "raw.binary_stream_not_emitted")
        adapter_gap = compat.capability_observation(
            None, "extract.stream_abi_unproven", absent
        )
        self.assertEqual(adapter_gap["status"], "adapter_evidence_gap")
        self.assertEqual(adapter_gap["failure_owner"], "adapter")

    def test_default_comparison_fails_closed_on_raw_and_bindings_drift(self):
        base = {
            "raw": {"client.rs": b"a"},
            "sdk": {"mod.rs": b"sdk"},
            "bindings": {"schema_version": 3, "operations": {}},
            "derivation": {"report": {"operations": {}}},
            "inventory": {"resources": []},
            "source_operations": [],
        }
        same = {
            key: (value.copy() if isinstance(value, dict) else value)
            for key, value in base.items()
        }
        self.assertTrue(compat.compare_default(base, same)["compatible"])

        changed = {
            key: (value.copy() if isinstance(value, dict) else value)
            for key, value in base.items()
        }
        changed["raw"] = {"client.rs": b"b"}
        result = compat.compare_default(base, changed)
        self.assertFalse(result["compatible"])
        self.assertEqual(result["raw"]["changed"], ["client.rs"])


if __name__ == "__main__":
    unittest.main()
