import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
GENERATOR_FIXTURE = ROOT / "tests" / "fixtures" / "menagerie"
BINDINGS_FIXTURE = (
    ROOT / "openapi-to-rust-bindings" / "tests" / "fixtures" / "menagerie"
)
BINDINGS_SHIM = Path(
    os.environ.get(
        "OPENAPI_TO_RUST_BINDINGS_BIN",
        ROOT / "target" / "debug" / "openapi-to-rust-bindings",
    )
)


class BindingsIntegrationTests(unittest.TestCase):
    def test_shim_requires_effective_openapi_and_never_reads_legacy_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            raw = Path(directory)
            (raw / "binding-manifest.json").write_text(
                (BINDINGS_FIXTURE / "binding-manifest.json").read_text()
            )
            (raw / "rust-bindings.json").write_text(
                (BINDINGS_FIXTURE / "rust-bindings.json").read_text()
            )

            missing = subprocess.run(
                [str(BINDINGS_SHIM), str(raw)],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(missing.returncode, 0)
            self.assertIn("adapter.input.effective_openapi_required", missing.stderr)
            self.assertFalse(missing.stdout)

            legacy_flag = subprocess.run(
                [str(BINDINGS_SHIM), "--legacy-metadata", str(raw)],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(legacy_flag.returncode, 0)
            self.assertIn("usage:", legacy_flag.stderr)
            self.assertFalse(legacy_flag.stdout)

            invalid = subprocess.run(
                [
                    str(BINDINGS_SHIM),
                    str(raw),
                    str(GENERATOR_FIXTURE / "openapi.json"),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(invalid.returncode, 0)
            self.assertIn("adapter.extract:", invalid.stderr)
            self.assertFalse(invalid.stdout)


if __name__ == "__main__":
    unittest.main()
