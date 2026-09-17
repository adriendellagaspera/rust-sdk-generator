import json
from pathlib import Path
import unittest

from rust_sdk_generator import Bindings

FIXTURES = Path(__file__).resolve().parent / "fixtures"


class BindingsSchemaTests(unittest.TestCase):
    def fixture(self) -> dict:
        return json.loads((FIXTURES / "menagerie" / "rust-bindings.json").read_text())

    def test_rejects_unknown_top_level_fields(self):
        value = self.fixture()
        value["unexpected"] = True
        with self.assertRaisesRegex(ValueError, "invalid Bindings"):
            Bindings.from_dict(value)

    def test_rejects_unknown_nested_fields(self):
        value = self.fixture()
        value["binding"]["client"]["unexpected"] = True
        with self.assertRaisesRegex(ValueError, "invalid Bindings"):
            Bindings.from_dict(value)

    def test_valid_fixture_still_round_trips(self):
        bindings = Bindings.from_dict(self.fixture())
        self.assertEqual(Bindings.from_dict(bindings.to_dict()), bindings)


if __name__ == "__main__":
    unittest.main()
