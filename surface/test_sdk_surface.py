from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from surface.sdk_surface import (
    ContractError,
    compatibility_report,
    compile_fern_input,
    semantic_inventory,
    tree_digest,
)


SOURCE = {
    "openapi": "3.1.0",
    "paths": {
        "/chat": {
            "post": {
                "operationId": "chat_complete",
                "tags": ["chat"],
                "responses": {"200": {"description": "ok"}},
            }
        },
        "/files": {
            "get": {
                "operationId": "files_list",
                "tags": ["beta.files"],
                "responses": {"200": {"description": "ok"}},
            }
        },
    },
    "components": {
        "schemas": {
            "ChatResponse": {"type": "object"},
            "Chunk": {"type": "object"},
            "JudgeOutput": {"type": "object"},
            "Judge": {
                "type": "object",
                "properties": {"output": {"type": "object"}},
            },
        }
    },
}

POLICY = {
    "schema_version": 1,
    "defaults": {
        "resource": "first_tag_segments",
        "method": "operation_id",
    },
    "operations": {
        "operationId:chat_complete": {
            "method": "complete",
            "streaming": {
                "format": "sse",
                "condition": "$request.stream",
                "response_schema": "ChatResponse",
                "stream_schema": "Chunk",
            },
        },
        "operationId:files_list": {
            "method": "list",
            "representation": "native-json",
        },
    },
    "types": {
        "#/components/schemas/Judge/properties/output": {
            "name": "JudgeOutputConfig"
        }
    },
}

IR = {
    "services": {
        "service_chat": {
            "name": {"fernFilepath": {"allParts": ["chat"]}},
            "endpoints": [{"name": "complete"}, {"name": "complete_stream"}],
        },
        "service_files": {
            "name": {"fernFilepath": {"allParts": ["beta", "files"]}},
            "endpoints": [{"name": "list"}],
        },
    },
    "types": {
        "ChatResponse": {
            "shape": {"_type": "named", "typeId": "Chunk"}
        },
        "Chunk": {},
        "JudgeOutput": {},
        "JudgeOutputConfig": {},
    },
}


class SurfaceContractTests(unittest.TestCase):
    def test_compiles_extensions_and_resolution(self) -> None:
        output, resolution = compile_fern_input(SOURCE, POLICY)
        chat = output["paths"]["/chat"]["post"]
        self.assertEqual(chat["x-fern-sdk-group-name"], ["chat"])
        self.assertEqual(chat["x-fern-sdk-method-name"], "complete")
        self.assertEqual(
            chat["x-fern-streaming"]["response-stream"]["$ref"],
            "#/components/schemas/Chunk",
        )
        inline = output["components"]["schemas"]["Judge"]["properties"]["output"]
        self.assertEqual(inline["x-fern-type-name"], "JudgeOutputConfig")
        self.assertEqual(resolution["source_operation_count"], 2)
        self.assertEqual(resolution["published_operation_count"], 2)

    def test_verifies_fern_ir_without_parsing_rust(self) -> None:
        inventory = semantic_inventory(SOURCE, POLICY, IR)
        self.assertEqual(
            inventory["verification"]["missing_public_methods"], []
        )
        self.assertEqual(
            inventory["verification"]["unexpected_public_methods"], []
        )
        self.assertEqual(
            inventory["verification"]["closure_missing_type_ids"], []
        )
        self.assertEqual(
            inventory["public_methods"],
            ["beta.files.list", "chat.complete", "chat.complete_stream"],
        )

    def test_rejects_public_method_collision(self) -> None:
        policy = json.loads(json.dumps(POLICY))
        policy["operations"]["operationId:files_list"]["resource"] = ["chat"]
        policy["operations"]["operationId:files_list"]["method"] = "complete"
        with self.assertRaises(ContractError):
            compile_fern_input(SOURCE, policy)

    def test_rejects_type_override_collision_with_component(self) -> None:
        policy = json.loads(json.dumps(POLICY))
        policy["types"][
            "#/components/schemas/Judge/properties/output"
        ]["name"] = "JudgeOutput"
        with self.assertRaises(ContractError):
            compile_fern_input(SOURCE, policy)

    def test_compatibility_marks_removed_or_remapped_surface_breaking(
        self,
    ) -> None:
        current = semantic_inventory(SOURCE, POLICY, IR)
        baseline = json.loads(json.dumps(current))
        baseline["public_methods"].append("chat.legacy")
        baseline["operations"][0]["public_paths"] = ["chat.legacy"]
        report = compatibility_report(current, baseline)
        self.assertTrue(report["breaking"])
        self.assertIn("chat.legacy", report["removed_public_methods"])
        self.assertTrue(report["changed_operation_mappings"])

    def test_tree_digest_ignores_fern_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src" / "lib.rs").write_text("pub fn x() {}\n")
            (root / ".fern").mkdir()
            (root / ".fern" / "volatile.json").write_text("one")
            first = tree_digest(root)
            (root / ".fern" / "volatile.json").write_text("two")
            second = tree_digest(root)
            self.assertEqual(first, second)


if __name__ == "__main__":
    unittest.main()
