import hashlib
import importlib.resources
import json
from pathlib import Path
import unittest

from jsonschema import Draft202012Validator

import rust_sdk_generator as package
from rust_sdk_generator import Bindings, OpenApi, Policy, compile


FIXTURES = Path(__file__).resolve().parent / "fixtures"
ORACLE = Path(__file__).resolve().parent / "oracle" / "current-generator.json"


def load_bindings(name: str) -> Bindings:
    value = json.loads((FIXTURES / name / "rust-bindings.json").read_text())
    schema = json.loads(
        importlib.resources.files(package).joinpath("rust-bindings.schema.json").read_text()
    )
    Draft202012Validator(schema).validate(value)
    return Bindings.from_dict(value)


def compile_fixture(name: str):
    root = FIXTURES / name
    return compile(
        OpenApi(json.loads((root / "openapi.json").read_text())),
        load_bindings(name),
        Policy.from_dict(json.loads((root / "policy.json").read_text())),
    )


def digest(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def current_oracle() -> dict[str, dict[str, str]]:
    return {
        fixture: {
            path: digest(source)
            for path, source in sorted(compile_fixture(fixture).files.items())
        }
        for fixture in ("library", "menagerie")
    }


class GeneratorOracleTests(unittest.TestCase):
    def test_current_generic_outputs_match_frozen_oracle(self):
        expected = json.loads(ORACLE.read_text())
        actual = current_oracle()
        self.assertEqual(expected, actual, json.dumps(actual, indent=2, sort_keys=True))


if __name__ == "__main__":
    unittest.main()
