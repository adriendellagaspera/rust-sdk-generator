import json
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
GENERATOR = Path(
    os.environ.get(
        "RUST_SDK_GENERATOR_BIN",
        ROOT / "target" / "debug" / "rust-sdk-generator",
    )
)
BINDINGS_ADAPTER = Path(
    os.environ.get(
        "OPENAPI_TO_RUST_BINDINGS_BIN",
        ROOT / "target" / "debug" / "openapi-to-rust-bindings",
    )
)


class BindingsIntegrationTests(unittest.TestCase):
    def test_adapter_sidecar_drives_rust_cli_deterministically(self):
        parsed_process = subprocess.run(
            [str(BINDINGS_ADAPTER), str(BINDINGS_FIXTURE)],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(parsed_process.returncode, 0, parsed_process.stderr)
        parsed = json.loads(parsed_process.stdout)
        expected = json.loads((GENERATOR_FIXTURE / "rust-bindings.json").read_text())
        for operation in expected["operations"].values():
            operation.setdefault("stream", None)
        self.assertEqual(parsed, expected)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bindings = root / "rust-bindings.json"
            bindings.write_text(json.dumps(parsed, sort_keys=True))

            snapshots = []
            inventories = []
            for name in ("first", "second"):
                output = root / name
                result = subprocess.run(
                    [
                        str(GENERATOR),
                        "generate",
                        "--openapi",
                        str(GENERATOR_FIXTURE / "openapi.json"),
                        "--bindings",
                        str(bindings),
                        "--definition",
                        str(GENERATOR_FIXTURE / "policy.json"),
                        "--output",
                        str(output),
                    ],
                    check=False,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                inventories.append(json.loads(result.stdout))
                snapshots.append(
                    {
                        path.relative_to(output).as_posix(): path.read_text()
                        for path in sorted(output.rglob("*"))
                        if path.is_file()
                    }
                )

            self.assertEqual(inventories[0], inventories[1])
            self.assertEqual(snapshots[0], snapshots[1])
            self.assertEqual(
                set(snapshots[0]), {"facade_types.rs", "zoo.rs", "mod.rs"}
            )
            self.assertIn("pub async fn adopt", snapshots[0]["zoo.rs"])


if __name__ == "__main__":
    unittest.main()
