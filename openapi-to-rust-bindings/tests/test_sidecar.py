import json
from pathlib import Path
import tempfile
import unittest

from openapi_to_rust_bindings import ParseError, read_bindings
from rust_sdk_generator import Bindings

FIXTURES = Path(__file__).resolve().parent / "fixtures"


class SidecarLoadingTests(unittest.TestCase):
    def expected(self, name: str) -> Bindings:
        return Bindings.from_dict(
            json.loads((FIXTURES / name / "rust-bindings.json").read_text())
        )

    def test_sidecar_does_not_require_generated_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.joinpath("rust-bindings.json").write_text(
                (FIXTURES / "menagerie" / "rust-bindings.json").read_text()
            )
            self.assertEqual(read_bindings(root), self.expected("menagerie"))

    def test_invalid_sidecar_fails_closed(self):
        fixture = FIXTURES / "menagerie"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.joinpath("rust-bindings.json").write_text("{not json")
            root.joinpath("types.rs").write_bytes((fixture / "types.rs").read_bytes())
            root.joinpath("client.rs").write_bytes((fixture / "client.rs").read_bytes())
            with self.assertRaises(ParseError):
                read_bindings(root)

    def test_schema_invalid_sidecar_fails_closed(self):
        value = json.loads((FIXTURES / "menagerie" / "rust-bindings.json").read_text())
        value["unexpected"] = True
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.joinpath("rust-bindings.json").write_text(json.dumps(value))
            with self.assertRaises(ParseError):
                read_bindings(root)

    def test_legacy_generated_sources_remain_supported(self):
        fixture = FIXTURES / "library"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.joinpath("types.rs").write_bytes((fixture / "types.rs").read_bytes())
            root.joinpath("client.rs").write_bytes((fixture / "client.rs").read_bytes())
            self.assertEqual(read_bindings(root), self.expected("library"))


if __name__ == "__main__":
    unittest.main()
