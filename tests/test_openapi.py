import json
from pathlib import Path
import unittest

from rust_sdk_generator import GenerationError, OpenApi


FIXTURE = Path(__file__).resolve().parent / "fixtures/composed-openapi/openapi.json"


class OpenApiTests(unittest.TestCase):
    def setUp(self):
        self.openapi = OpenApi(json.loads(FIXTURE.read_text()))

    def test_object_schema_flattens_local_allof_recursively(self):
        schema = self.openapi.object_schema("NestedCommand")
        self.assertEqual(schema["type"], "object")
        self.assertEqual(list(schema["properties"]), ["name", "metadata", "dry_run", "priority"])
        self.assertEqual(schema["required"], ["name", "priority"])
        self.assertFalse(schema["additionalProperties"])
        self.assertEqual(schema["properties"]["dry_run"], {"type": "boolean", "enum": [False]})

    def test_object_schema_intersects_compatible_literal_refinement(self):
        schema = self.openapi.object_schema("NonStreamingCommand")
        self.assertEqual(
            schema["properties"]["stream"],
            {"type": "boolean", "enum": [False], "default": False},
        )

    def test_object_schema_intersects_nullable_non_null_refinement(self):
        schema = self.openapi.object_schema("NonStreamingNullableCommand")
        self.assertEqual(
            schema["properties"]["stream"],
            {"type": "boolean", "enum": [False], "default": False},
        )

    def test_object_schema_rejects_incompatible_nullable_refinement(self):
        with self.assertRaisesRegex(
            GenerationError, "conflicting OpenAPI property InvalidNullableRefinement.stream"
        ):
            self.openapi.object_schema("InvalidNullableRefinement")

    def test_object_schema_rejects_empty_literal_intersection(self):
        with self.assertRaisesRegex(
            GenerationError, "conflicting OpenAPI property ImpossibleCommand.mode"
        ):
            self.openapi.object_schema("ImpossibleCommand")

    def test_object_schema_rejects_conflicting_allof_properties(self):
        with self.assertRaisesRegex(GenerationError, "conflicting OpenAPI property Conflict.value"):
            self.openapi.object_schema("Conflict")

    def test_object_schema_rejects_undefined_required_properties(self):
        with self.assertRaisesRegex(GenerationError, "requires undefined properties"):
            self.openapi.object_schema("BrokenRequired")

    def test_object_schema_rejects_recursive_composition(self):
        with self.assertRaisesRegex(GenerationError, "recursive OpenAPI object composition"):
            self.openapi.object_schema("RecursiveA")


if __name__ == "__main__":
    unittest.main()