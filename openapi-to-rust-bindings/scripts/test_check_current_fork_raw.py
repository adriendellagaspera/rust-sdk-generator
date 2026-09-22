"""Transitional fork watch must fail closed without hiding moving-candidate drift."""

from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

import check_current_fork_raw as watch


HISTORICAL_SHA = "a" * 40
CANDIDATE_SHA = "b" * 40
BINDINGS = {
    "schema_version": 3,
    "operations": {
        "fetch": {
            "return_type": "Result< (), Error,>",
            "metadata": {
                "source_operation": {
                    "operation_id": "fetch",
                    "method": "GET",
                    "path": "/resource",
                },
                "stream_abi": None,
            },
        }
    },
}


class ForkRawWatchTests(unittest.TestCase):
    def test_config_removes_only_obsolete_manifest_producer_flag(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture = root / "fixture"
            fixture.mkdir()
            (fixture / "compat.toml").write_text(
                '[generator]\nspec_path = "openapi.json"\n'
                'binding_manifest = true\nmodule_name = "fixture"\n'
                '\n[features]\nenable_async_client = true\n'
            )
            (fixture / "openapi.json").write_text('{"paths":{}}')
            archive, archive_spec = watch.prepare(fixture, root / "historical", True)
            moving, moving_spec = watch.prepare(fixture, root / "moving", False)
            self.assertIn("binding_manifest = true", archive.read_text())
            self.assertNotIn("binding_manifest", moving.read_text())
            self.assertIn("enable_async_client = true", moving.read_text())
            self.assertEqual(archive_spec.read_bytes(), moving_spec.read_bytes())

    def test_rejects_silently_missing_source_operation(self):
        spec = {"paths": {"/resource": {"get": {"operationId": "fetch"}},
                          "/missing": {"post": {"operationId": "missing"}}}}
        with self.assertRaisesRegex(ValueError, "unemitted"):
            watch.assert_source_coverage(spec, watch.canonical(BINDINGS))

    def test_main_reports_stage_and_refuses_silent_candidate_raw_drift(self):
        for drift in (False, True):
            with self.subTest(drift=drift):
                with tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    fixture = root / "fixture"
                    fixture.mkdir()
                    (fixture / "compat.toml").write_text(
                        '[generator]\nspec_path = "openapi.json"\n'
                        'binding_manifest = true\noutput_dir = "raw"\n'
                    )
                    (fixture / "openapi.json").write_text(json.dumps({
                        "openapi": "3.1.0",
                        "paths": {"/resource": {"get": {"operationId": "fetch"}}},
                    }))
                    report = root / "report.json"
                    report_md = root / "report.md"
                    arguments = [
                        "check_current_fork_raw.py",
                        "--historical-generator", str(root / "old"),
                        "--candidate-generator", str(root / "new"),
                        "--historical-sha", HISTORICAL_SHA,
                        "--candidate-sha", CANDIDATE_SHA,
                        "--adapter", str(root / "adapter"),
                        "--fixture", str(fixture),
                        "--report-json", str(report),
                        "--report-md", str(report_md),
                    ]

                    def fake_command(*args, cwd=None):
                        if len(args) > 1 and args[1] == "generate":
                            assert cwd is not None
                            raw = cwd / "raw"
                            raw.mkdir()
                            for name in ("client.rs", "types.rs", "mod.rs", "REQUIRED_DEPS.toml"):
                                raw.joinpath(name).write_text(
                                    "changed" if drift and cwd.name == "candidate"
                                    and name == "client.rs" else "fixed"
                                )
                            if cwd.name == "historical":
                                (raw / "binding-manifest.json").write_text("{}")
                            return ""
                        return json.dumps(BINDINGS)

                    with (
                        mock.patch.object(sys, "argv", arguments),
                        mock.patch.object(watch, "command", side_effect=fake_command),
                    ):
                        status = watch.main()
                    output = json.loads(report.read_text())
                    self.assertEqual(status, 1 if drift else 0)
                    self.assertEqual(output["compatible"], not drift)
                    self.assertEqual(output["stage"], "canonical_and_raw_parity" if drift else "passed")
                    self.assertEqual(output["moving_fork_sha"], CANDIDATE_SHA)
                    self.assertEqual(output["effective_openapi_sha256"],
                                     watch.hashlib.sha256((fixture / "openapi.json").read_bytes()).hexdigest())
                    if drift:
                        self.assertEqual(output["changed_raw_files"], ["client.rs"])
                        self.assertIn("review files", output["diagnostic"])
                    else:
                        self.assertEqual(output["changed_raw_files"], [])
                    self.assertTrue(report_md.is_file())

    def test_invalid_pin_reports_failure_without_running_generators(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            report = root / "report.json"
            report_md = root / "report.md"
            arguments = [
                "check_current_fork_raw.py",
                "--historical-generator", str(root / "old"),
                "--candidate-generator", str(root / "new"),
                "--historical-sha", "moving-branch",
                "--candidate-sha", CANDIDATE_SHA,
                "--adapter", str(root / "adapter"),
                "--fixture", str(root / "missing"),
                "--report-json", str(report),
                "--report-md", str(report_md),
            ]
            with mock.patch.object(sys, "argv", arguments):
                self.assertEqual(watch.main(), 1)
            output = json.loads(report.read_text())
            self.assertEqual(output["stage"], "configuration")
            self.assertIn("immutable", output["diagnostic"])


if __name__ == "__main__":
    unittest.main()
