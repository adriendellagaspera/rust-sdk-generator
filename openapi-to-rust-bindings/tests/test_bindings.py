import json
from pathlib import Path
import tomllib
import unittest

import openapi_to_rust_bindings as package
from openapi_to_rust_bindings import Bindings, parse_bindings, read_bindings

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
FIXTURES = Path(__file__).resolve().parent / "fixtures"


class BindingsPackageTests(unittest.TestCase):
    def test_public_api_is_functional_and_small(self):
        self.assertEqual(
            set(package.__all__),
            {"Bindings", "ParseError", "parse_bindings", "read_bindings"},
        )

    def test_tests_use_installed_package_not_source_tree(self):
        self.assertNotEqual(
            Path(package.__file__).resolve().parent,
            SRC / "openapi_to_rust_bindings",
        )

    def test_fixtures_match_checked_in_bindings_contract(self):
        for name in ("menagerie", "library"):
            with self.subTest(name=name):
                root = FIXTURES / name
                expected = Bindings.from_dict(
                    json.loads((root / "rust-bindings.json").read_text())
                )
                actual = parse_bindings(
                    (root / "types.rs").read_bytes(),
                    (root / "client.rs").read_bytes(),
                )
                self.assertIsInstance(actual, Bindings)
                self.assertEqual(actual, expected)
                self.assertEqual(read_bindings(root), expected)

    def test_generated_serde_rename_is_preserved(self):
        bindings = parse_bindings(
            '''
pub enum State {
    #[serde(rename = "in-progress")]
    InProgress,
}
''',
            "",
        )
        self.assertEqual(
            bindings.to_dict()["enums"]["State"][0]["wire_name"],
            "in-progress",
        )

    def test_package_does_not_import_compiler_runtime(self):
        for name in ("model.py", "parser.py", "reader.py"):
            source = (SRC / "openapi_to_rust_bindings" / name).read_text()
            self.assertNotIn("rust_sdk_generator", source)
            self.assertNotIn("RawIr", source)
            self.assertNotIn("Adapter", source)

    def test_metadata_pins_backend_contract_without_python_compiler_dependency(self):
        project = tomllib.loads((ROOT / "pyproject.toml").read_text())["project"]
        self.assertEqual(project["name"], "openapi-to-rust-bindings")
        self.assertEqual(project["version"], package.__version__)
        self.assertIn("jsonschema==4.23.0", project["dependencies"])
        self.assertFalse(
            any(dependency.startswith("rust-sdk-generator") for dependency in project["dependencies"])
        )
        compatibility = json.loads((ROOT / "COMPATIBILITY.json").read_text())
        self.assertEqual(compatibility["package_version"], package.__version__)
        self.assertEqual(compatibility["bindings_schema_version"], 2)
        self.assertEqual(compatibility["sidecar"], "rust-bindings.json")
        self.assertEqual(
            compatibility["backend"]["commit"],
            "2af34b86ca9f38c35787f13ec5841989efcf4b99",
        )


if __name__ == "__main__":
    unittest.main()
