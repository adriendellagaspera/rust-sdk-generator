"""Actual legacy compatibility report tests, including fail-closed stage failures."""

from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

import check_backend_compat as compat


PIN = "d19e5a4cba2589475dc157e50fe624576f129368"


class CompatibilityStageTests(unittest.TestCase):
    def test_report_classifies_raw_adapter_and_nonexecuted_root_stages(self):
        for failing_stage in ("raw_generation", "adapter_evidence", None):
            with self.subTest(failing_stage=failing_stage):
                with tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    package = root / "package"
                    fixtures = package / "tests" / "fixtures"
                    fixture = fixtures / "oracle"
                    fixture.mkdir(parents=True)
                    (fixtures / "compatibility-cases.json").write_text(
                        json.dumps({"schema_version": 1, "legacy_manifest_oracle": ["oracle"]})
                    )
                    (fixture / "openapi.json").write_text('{"openapi":"3.1.0","paths":{}}')
                    (fixture / "binding-manifest.json").write_text("{}")
                    (package / "COMPATIBILITY.json").write_text(json.dumps({
                        "schema_version": 2,
                        "bindings_schema_version": 3,
                        "backend": {
                            "name": "openapi-to-rust",
                            "repository": "adriendellagaspera/openapi-to-rust",
                            "baseline": {"version": "0.17.0", "commit": PIN},
                        },
                    }))
                    args = [
                        "check_backend_compat.py", "--package-root", str(package),
                        "--bindings-adapter", str(root / "adapter"),
                        "--baseline-generator", str(root / "baseline"),
                        "--candidate-generator", str(root / "candidate"),
                        "--candidate-commit", PIN,
                        "--report", str(root / "report.md"),
                        "--report-json", str(root / "report.json"),
                    ]

                    def generate(binary: Path, _fixture: Path, destination: Path) -> None:
                        if failing_stage == "raw_generation" and binary.name == "candidate":
                            raise RuntimeError("producer manifest flag is unsupported")
                        destination.mkdir(parents=True)
                        (destination / "client.rs").write_text("ordinary rust")

                    def adapt(_adapter: Path, generated: Path) -> dict:
                        if failing_stage == "adapter_evidence" and "candidate" in generated.parts:
                            raise RuntimeError("extract.representation_unproven")
                        return {"schema_version": 3, "operations": {}}

                    with (
                        mock.patch.object(sys, "argv", args),
                        mock.patch.object(compat, "generator_version", return_value="0.17.0"),
                        mock.patch.object(compat, "generate_fixture", side_effect=generate),
                        mock.patch.object(compat, "bindings_value", side_effect=adapt),
                    ):
                        if failing_stage is None:
                            compat.main()
                        else:
                            with self.assertRaises(SystemExit) as error:
                                compat.main()
                            self.assertEqual(error.exception.code, 1)

                    result = json.loads((root / "report.json").read_text())
                    row = result["fixture_results"][0]
                    self.assertEqual(result["profile"], "legacy_manifest_oracle")
                    self.assertEqual(result["compatible"], failing_stage is None)
                    self.assertEqual(row["failure_stage"], failing_stage)
                    self.assertEqual(row["stages"]["root_sdk_derivation"], "not_run_legacy_oracle")
                    self.assertEqual(result["rejected_operations"]["status"], "not_run_legacy_oracle")
                    self.assertIn("source_openapi_sha256", row["provenance"]["candidate"])
                    if failing_stage == "raw_generation":
                        self.assertEqual(row["provenance"]["candidate"]["effective_openapi_status"], "unavailable_generation_failed")
                        self.assertIsNone(row["provenance"]["candidate"]["effective_openapi_sha256"])
                    self.assertEqual(result["backend"]["candidate"]["commit"], PIN)
                    self.assertIn("historical manifest oracle", (root / "report.md").read_text())

    def test_baseline_failure_is_reported_without_false_drift_or_crash(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            package = root / "package"
            fixture = package / "tests" / "fixtures" / "oracle"
            fixture.mkdir(parents=True)
            (fixture.parent / "compatibility-cases.json").write_text(
                json.dumps({"schema_version": 1, "legacy_manifest_oracle": ["oracle"]})
            )
            (fixture / "openapi.json").write_text("{}")
            (fixture / "binding-manifest.json").write_text("{}")
            (package / "COMPATIBILITY.json").write_text(json.dumps({
                "schema_version": 2, "bindings_schema_version": 3,
                "backend": {"name": "openapi-to-rust",
                            "repository": "adriendellagaspera/openapi-to-rust",
                            "baseline": {"version": "0.17.0", "commit": PIN}},
            }))

            def generate(binary: Path, _fixture: Path, destination: Path) -> None:
                if binary.name == "baseline":
                    raise RuntimeError("baseline raw failure")
                destination.mkdir(parents=True)
                (destination / "client.rs").write_text("candidate")

            args = ["check_backend_compat.py", "--package-root", str(package),
                    "--bindings-adapter", str(root / "adapter"),
                    "--baseline-generator", str(root / "baseline"),
                    "--candidate-generator", str(root / "candidate"),
                    "--candidate-commit", PIN, "--report", str(root / "report.md"),
                    "--report-json", str(root / "report.json")]
            with (
                mock.patch.object(sys, "argv", args),
                mock.patch.object(compat, "generator_version", return_value="0.17.0"),
                mock.patch.object(compat, "generate_fixture", side_effect=generate),
                mock.patch.object(compat, "bindings_value", return_value={"schema_version": 3, "operations": {}}),
            ):
                with self.assertRaises(SystemExit):
                    compat.main()
            row = json.loads((root / "report.json").read_text())["fixture_results"][0]
            self.assertEqual(row["failure_stage"], "raw_generation")
            self.assertEqual(row["source_added"], [])
            self.assertEqual(row["raw_changed"], [])
            self.assertEqual(row["bindings_changed"], [])


if __name__ == "__main__":
    unittest.main()
