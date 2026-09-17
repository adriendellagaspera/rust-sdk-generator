import importlib.util
import json
from pathlib import Path
import unittest

from rust_sdk_generator import Bindings, OpenApi, Policy, compile


ROOT = Path(__file__).resolve().parents[1]
GENERATOR_FIXTURE = ROOT / "tests" / "fixtures" / "menagerie"
BINDINGS_FIXTURE = (
    ROOT / "openapi-to-rust-bindings" / "tests" / "fixtures" / "menagerie"
)
HAS_BINDINGS_PACKAGE = importlib.util.find_spec("openapi_to_rust_bindings") is not None


@unittest.skipUnless(HAS_BINDINGS_PACKAGE, "bindings component not installed")
class BindingsIntegrationTests(unittest.TestCase):
    def test_generated_rust_normalizes_into_generator_contract(self):
        from openapi_to_rust_bindings import parse_bindings

        parsed = parse_bindings(
            (BINDINGS_FIXTURE / "types.rs").read_bytes(),
            (BINDINGS_FIXTURE / "client.rs").read_bytes(),
        )
        expected = Bindings.from_dict(
            json.loads((GENERATOR_FIXTURE / "rust-bindings.json").read_text())
        )
        self.assertEqual(parsed, expected)

        openapi = OpenApi(
            json.loads((GENERATOR_FIXTURE / "openapi.json").read_text())
        )
        policy = Policy.from_dict(
            json.loads((GENERATOR_FIXTURE / "policy.json").read_text())
        )
        first = compile(openapi, parsed, policy)
        second = compile(openapi, parsed, policy)

        self.assertEqual(dict(first.files), dict(second.files))
        self.assertEqual(
            set(first.files), {"facade_types.rs", "zoo.rs", "mod.rs"}
        )
        self.assertIn("pub async fn adopt", first.files["zoo.rs"])


if __name__ == "__main__":
    unittest.main()
