import json
from pathlib import Path
import unittest

from openapi_to_rust_bindings import parse_bindings
from rust_sdk_generator import Bindings, OpenApi, Policy, compile

ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT / "openapi-to-rust-bindings" / "tests" / "fixtures" / "menagerie"
SDK = ROOT / "tests" / "fixtures" / "menagerie"


class EndToEndTests(unittest.TestCase):
    def test_openapi_to_rust_sources_flow_through_bindings_and_generator(self):
        bindings = parse_bindings(
            (RAW / "types.rs").read_bytes(),
            (RAW / "client.rs").read_bytes(),
        )
        expected = Bindings.from_dict(json.loads((RAW / "rust-bindings.json").read_text()))
        self.assertEqual(expected, bindings)

        openapi = OpenApi(json.loads((SDK / "openapi.json").read_text()))
        policy = Policy.from_dict(json.loads((SDK / "policy.json").read_text()))
        first = compile(openapi, bindings, policy)
        second = compile(openapi, bindings, policy)

        self.assertEqual(dict(first.files), dict(second.files))
        self.assertEqual(first.ir.client_name, "Menagerie")
        self.assertEqual(set(first.files), {"facade_types.rs", "zoo.rs", "mod.rs"})
        self.assertIn("pub async fn adopt(&self, request: Adoption)", first.files["zoo.rs"])


if __name__ == "__main__":
    unittest.main()
