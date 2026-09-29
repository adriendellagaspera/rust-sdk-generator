#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

import yaml

HTTP_METHODS = {"get", "put", "post", "delete", "options", "head", "patch", "trace"}
POLICY_VERSION = 1


class ContractError(ValueError):
    pass


def load_document(path: Path) -> Any:
    text = path.read_text()
    if path.suffix.lower() == ".json":
        return json.loads(text)
    return yaml.safe_load(text)


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def operation_identity(operation: dict[str, Any], method: str, path: str) -> str:
    operation_id = operation.get("operationId")
    if isinstance(operation_id, str) and operation_id:
        return f"operationId:{operation_id}"
    return f"http:{method.upper()} {path}"


@dataclass(frozen=True)
class SourceOperation:
    identity: str
    method: str
    path: str
    operation_id: str | None
    tags: tuple[str, ...]


@dataclass(frozen=True)
class ResolvedOperation:
    source: SourceOperation
    outcome: str
    resource: tuple[str, ...]
    method_name: str | None
    representation: str | None
    streaming: dict[str, Any] | None
    exclusion_reason: str | None

    @property
    def public_paths(self) -> tuple[tuple[str, ...], ...]:
        if self.outcome != "published" or self.method_name is None:
            return ()
        base = self.resource + (self.method_name,)
        if self.streaming is None:
            return (base,)
        return (base, self.resource + (f"{self.method_name}_stream",))


def source_operations(spec: dict[str, Any]) -> list[SourceOperation]:
    operations: list[SourceOperation] = []
    for path, path_item in (spec.get("paths") or {}).items():
        if not isinstance(path_item, dict):
            continue
        for method, operation in path_item.items():
            if method.lower() not in HTTP_METHODS or not isinstance(operation, dict):
                continue
            tags = tuple(tag for tag in operation.get("tags", []) if isinstance(tag, str))
            operation_id = operation.get("operationId")
            if not isinstance(operation_id, str):
                operation_id = None
            operations.append(
                SourceOperation(
                    identity=operation_identity(operation, method, path),
                    method=method.upper(),
                    path=path,
                    operation_id=operation_id,
                    tags=tags,
                )
            )
    identities = [item.identity for item in operations]
    duplicates = sorted({identity for identity in identities if identities.count(identity) > 1})
    if duplicates:
        raise ContractError(f"duplicate source operation identities: {duplicates}")
    return sorted(operations, key=lambda item: (item.path, item.method, item.identity))


def validate_policy(policy: dict[str, Any]) -> None:
    if not isinstance(policy, dict):
        raise ContractError("policy must be an object")
    if policy.get("schema_version") != POLICY_VERSION:
        raise ContractError(f"policy schema_version must be {POLICY_VERSION}")
    defaults = policy.get("defaults", {})
    if not isinstance(defaults, dict):
        raise ContractError("policy defaults must be an object")
    if defaults.get("resource") not in {None, "first_tag_segments"}:
        raise ContractError("defaults.resource must be first_tag_segments when set")
    if defaults.get("method") not in {None, "operation_id"}:
        raise ContractError("defaults.method must be operation_id when set")
    operations = policy.get("operations", {})
    if not isinstance(operations, dict):
        raise ContractError("policy operations must be an object keyed by source identity")
    types = policy.get("types", {})
    if not isinstance(types, dict):
        raise ContractError("policy types must be an object keyed by JSON pointer")

    for identity, entry in operations.items():
        if not isinstance(identity, str) or not identity:
            raise ContractError("operation policy keys must be non-empty strings")
        if not isinstance(entry, dict):
            raise ContractError(f"operation policy {identity!r} must be an object")
        exclude = entry.get("exclude")
        if exclude is not None:
            if not isinstance(exclude, dict) or not isinstance(exclude.get("reason"), str):
                raise ContractError(f"operation policy {identity!r} exclude.reason is required")
            forbidden = {"resource", "method", "streaming"}.intersection(entry)
            if forbidden:
                raise ContractError(
                    f"excluded operation {identity!r} cannot also define {sorted(forbidden)}"
                )
        resource = entry.get("resource")
        if resource is not None and (
            not isinstance(resource, list)
            or not resource
            or any(not isinstance(part, str) or not part for part in resource)
        ):
            raise ContractError(
                f"operation policy {identity!r} resource must be a non-empty string list"
            )
        method = entry.get("method")
        if method is not None and (not isinstance(method, str) or not method):
            raise ContractError(
                f"operation policy {identity!r} method must be a non-empty string"
            )
        streaming = entry.get("streaming")
        if streaming is not None:
            if not isinstance(streaming, dict) or streaming.get("format") != "sse":
                raise ContractError(
                    f"operation policy {identity!r} supports only streaming.format=sse"
                )
            required = ["condition", "response_schema", "stream_schema"]
            missing = [
                key
                for key in required
                if not isinstance(streaming.get(key), str) or not streaming[key]
            ]
            if missing:
                raise ContractError(
                    f"operation policy {identity!r} streaming missing {missing}"
                )

    for pointer, entry in types.items():
        if not isinstance(pointer, str) or not pointer.startswith("#/"):
            raise ContractError(
                f"type policy key {pointer!r} must be a local JSON pointer"
            )
        if not isinstance(entry, dict):
            raise ContractError(f"type policy {pointer!r} must be an object")
        name = entry.get("name")
        if name is not None and (not isinstance(name, str) or not name):
            raise ContractError(
                f"type policy {pointer!r} name must be a non-empty string"
            )


def default_resource(
    operation: SourceOperation, policy: dict[str, Any]
) -> tuple[str, ...] | None:
    if policy.get("defaults", {}).get("resource") != "first_tag_segments":
        return None
    if not operation.tags:
        return None
    return tuple(part for part in operation.tags[0].split(".") if part)


def default_method(operation: SourceOperation, policy: dict[str, Any]) -> str | None:
    if policy.get("defaults", {}).get("method") != "operation_id":
        return None
    return operation.operation_id


def resolve_policy(
    spec: dict[str, Any], policy: dict[str, Any]
) -> list[ResolvedOperation]:
    validate_policy(policy)
    operations = source_operations(spec)
    by_identity = {operation.identity: operation for operation in operations}
    unknown = sorted(set(policy.get("operations", {})) - set(by_identity))
    if unknown:
        raise ContractError(
            f"policy operation identities not found in source: {unknown}"
        )

    resolved: list[ResolvedOperation] = []
    for operation in operations:
        entry = policy.get("operations", {}).get(operation.identity, {})
        exclude = entry.get("exclude")
        if exclude is not None:
            resolved.append(
                ResolvedOperation(
                    source=operation,
                    outcome="excluded",
                    resource=(),
                    method_name=None,
                    representation=entry.get("representation"),
                    streaming=None,
                    exclusion_reason=exclude["reason"],
                )
            )
            continue

        resource = tuple(
            entry.get("resource") or default_resource(operation, policy) or ()
        )
        method_name = entry.get("method") or default_method(operation, policy)
        if not resource or not method_name:
            missing = []
            if not resource:
                missing.append("resource")
            if not method_name:
                missing.append("method")
            raise ContractError(
                f"source operation {operation.identity!r} has no reviewed/default "
                f"resolution for {missing}"
            )
        resolved.append(
            ResolvedOperation(
                source=operation,
                outcome="published",
                resource=resource,
                method_name=method_name,
                representation=entry.get("representation"),
                streaming=entry.get("streaming"),
                exclusion_reason=None,
            )
        )

    collisions: dict[tuple[str, ...], list[str]] = {}
    for operation in resolved:
        for public_path in operation.public_paths:
            collisions.setdefault(public_path, []).append(operation.source.identity)
    duplicate_paths = {
        ".".join(path): identities
        for path, identities in collisions.items()
        if len(set(identities)) > 1
    }
    if duplicate_paths:
        raise ContractError(f"public method collisions: {duplicate_paths}")
    validate_type_name_collisions(spec, policy)
    return resolved


def decode_pointer(pointer: str) -> list[str]:
    return [
        part.replace("~1", "/").replace("~0", "~")
        for part in pointer[2:].split("/")
    ]


def pointer_target(document: dict[str, Any], pointer: str) -> dict[str, Any]:
    current: Any = document
    for part in decode_pointer(pointer):
        if not isinstance(current, dict) or part not in current:
            raise ContractError(f"type policy pointer not found in source: {pointer}")
        current = current[part]
    if not isinstance(current, dict):
        raise ContractError(
            f"type policy pointer must target an object schema: {pointer}"
        )
    return current


def component_names(spec: dict[str, Any]) -> set[str]:
    schemas = ((spec.get("components") or {}).get("schemas") or {})
    if not isinstance(schemas, dict):
        return set()
    return set(schemas)


def validate_type_name_collisions(
    spec: dict[str, Any], policy: dict[str, Any]
) -> None:
    components = component_names(spec)
    owners: dict[str, str] = {
        name: f"#/components/schemas/{name}" for name in components
    }
    for pointer, entry in policy.get("types", {}).items():
        pointer_target(spec, pointer)
        public_name = entry.get("name")
        if public_name is None:
            continue
        previous = owners.get(public_name)
        if previous is not None and previous != pointer:
            raise ContractError(
                f"public type collision for {public_name!r}: "
                f"{previous!r} and {pointer!r}"
            )
        owners[public_name] = pointer


def resolution_inventory(
    resolved: Iterable[ResolvedOperation], policy: dict[str, Any]
) -> dict[str, Any]:
    operations = []
    for item in resolved:
        operations.append(
            {
                "source_identity": item.source.identity,
                "http_method": item.source.method,
                "http_path": item.source.path,
                "operation_id": item.source.operation_id,
                "outcome": item.outcome,
                "public_paths": [".".join(path) for path in item.public_paths],
                "representation": item.representation,
                "exclusion_reason": item.exclusion_reason,
            }
        )
    type_policy = [
        {
            "source_schema": pointer,
            "public_name": entry.get("name"),
            "representation": entry.get("representation"),
        }
        for pointer, entry in sorted(policy.get("types", {}).items())
    ]
    return {
        "schema_version": POLICY_VERSION,
        "source_operation_count": len(operations),
        "published_operation_count": sum(
            item["outcome"] == "published" for item in operations
        ),
        "excluded_operation_count": sum(
            item["outcome"] == "excluded" for item in operations
        ),
        "operations": operations,
        "type_policy": type_policy,
    }


def compile_fern_input(
    spec: dict[str, Any], policy: dict[str, Any]
) -> tuple[dict[str, Any], dict[str, Any]]:
    resolved = resolve_policy(spec, policy)
    output = json.loads(json.dumps(spec))
    resolved_by_key = {
        (item.source.path, item.source.method.lower()): item for item in resolved
    }

    for path, path_item in list((output.get("paths") or {}).items()):
        if not isinstance(path_item, dict):
            continue
        for method in list(path_item):
            key = (path, method.lower())
            if method.lower() not in HTTP_METHODS or key not in resolved_by_key:
                continue
            item = resolved_by_key[key]
            if item.outcome == "excluded":
                del path_item[method]
                continue
            operation = path_item[method]
            operation["x-fern-sdk-group-name"] = list(item.resource)
            operation["x-fern-sdk-method-name"] = item.method_name
            if item.streaming is not None:
                streaming = item.streaming
                operation["x-fern-streaming"] = {
                    "format": "sse",
                    "stream-condition": streaming["condition"],
                    "response": {
                        "$ref": (
                            "#/components/schemas/"
                            f"{streaming['response_schema']}"
                        )
                    },
                    "response-stream": {
                        "$ref": (
                            "#/components/schemas/"
                            f"{streaming['stream_schema']}"
                        )
                    },
                }

    for pointer, entry in policy.get("types", {}).items():
        if entry.get("name") is not None:
            pointer_target(output, pointer)["x-fern-type-name"] = entry["name"]

    return output, resolution_inventory(resolved, policy)


def fern_resource_path(service: dict[str, Any]) -> tuple[str, ...]:
    name = service.get("name") or {}
    filepath = name.get("fernFilepath") or {}
    parts = filepath.get("allParts") or []
    result: list[str] = []
    for part in parts:
        if isinstance(part, str):
            result.extend(piece for piece in part.split(".") if piece)
    return tuple(result)


def fern_public_method_paths(ir: dict[str, Any]) -> set[tuple[str, ...]]:
    result: set[tuple[str, ...]] = set()
    for service in (ir.get("services") or {}).values():
        if not isinstance(service, dict):
            continue
        resource = fern_resource_path(service)
        for endpoint in service.get("endpoints") or []:
            if not isinstance(endpoint, dict):
                continue
            name = endpoint.get("name")
            if isinstance(name, str) and name:
                result.add(resource + (name,))
    return result


def named_type_refs(value: Any) -> set[str]:
    refs: set[str] = set()
    if isinstance(value, dict):
        if value.get("_type") == "named" and isinstance(value.get("typeId"), str):
            refs.add(value["typeId"])
        for child in value.values():
            refs.update(named_type_refs(child))
    elif isinstance(value, list):
        for child in value:
            refs.update(named_type_refs(child))
    return refs


def verify_closure(ir: dict[str, Any]) -> list[str]:
    types = ir.get("types") or {}
    if not isinstance(types, dict):
        raise ContractError("Fern IR types must be an object")
    available = set(types)
    refs = named_type_refs(ir.get("services") or {}) | named_type_refs(types)
    return sorted(refs - available)


def ir_public_type_names(ir: dict[str, Any]) -> list[str]:
    types = ir.get("types") or {}
    if not isinstance(types, dict):
        raise ContractError("Fern IR types must be an object")
    return sorted(types)


def semantic_inventory(
    spec: dict[str, Any], policy: dict[str, Any], ir: dict[str, Any]
) -> dict[str, Any]:
    resolved = resolve_policy(spec, policy)
    resolution = resolution_inventory(resolved, policy)
    expected = {path for item in resolved for path in item.public_paths}
    actual = fern_public_method_paths(ir)
    missing = sorted(".".join(path) for path in expected - actual)
    unexpected = sorted(".".join(path) for path in actual - expected)
    closure_missing = verify_closure(ir)

    public_types = ir_public_type_names(ir)
    public_type_set = set(public_types)
    type_provenance = []
    for component in sorted(component_names(spec)):
        pointer = f"#/components/schemas/{component}"
        override = policy.get("types", {}).get(pointer, {}).get("name")
        public_name = override or component
        type_provenance.append(
            {
                "source_schema": pointer,
                "public_name": (
                    public_name if public_name in public_type_set else None
                ),
            }
        )
    for pointer, entry in sorted(policy.get("types", {}).items()):
        if pointer.startswith("#/components/schemas/") and pointer.count("/") == 3:
            continue
        public_name = entry.get("name")
        type_provenance.append(
            {
                "source_schema": pointer,
                "public_name": (
                    public_name if public_name in public_type_set else None
                ),
            }
        )

    return {
        **resolution,
        "public_method_count": len(actual),
        "public_methods": sorted(".".join(path) for path in actual),
        "public_type_count": len(public_types),
        "public_types": public_types,
        "provenance": {
            "operations": [
                {
                    "source_identity": item.source.identity,
                    "public_paths": [
                        ".".join(path) for path in item.public_paths
                    ],
                }
                for item in resolved
            ],
            "types": type_provenance,
        },
        "verification": {
            "missing_public_methods": missing,
            "unexpected_public_methods": unexpected,
            "closure_missing_type_ids": closure_missing,
        },
    }


def compatibility_report(
    current: dict[str, Any], baseline: dict[str, Any]
) -> dict[str, Any]:
    current_methods = set(current.get("public_methods", []))
    baseline_methods = set(baseline.get("public_methods", []))
    current_types = set(current.get("public_types", []))
    baseline_types = set(baseline.get("public_types", []))

    current_mapping = {
        item["source_identity"]: tuple(item.get("public_paths", []))
        for item in current.get("operations", [])
    }
    baseline_mapping = {
        item["source_identity"]: tuple(item.get("public_paths", []))
        for item in baseline.get("operations", [])
    }
    changed_mappings = []
    for identity in sorted(set(current_mapping) & set(baseline_mapping)):
        if current_mapping[identity] != baseline_mapping[identity]:
            changed_mappings.append(
                {
                    "source_identity": identity,
                    "before": list(baseline_mapping[identity]),
                    "after": list(current_mapping[identity]),
                }
            )

    report = {
        "removed_public_methods": sorted(
            baseline_methods - current_methods
        ),
        "added_public_methods": sorted(current_methods - baseline_methods),
        "removed_public_types": sorted(baseline_types - current_types),
        "added_public_types": sorted(current_types - baseline_types),
        "changed_operation_mappings": changed_mappings,
    }
    report["breaking"] = bool(
        report["removed_public_methods"]
        or report["removed_public_types"]
        or report["changed_operation_mappings"]
    )
    return report


def tree_digest(root: Path) -> dict[str, Any]:
    files = []
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        rel = path.relative_to(root)
        if ".fern" in rel.parts:
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        files.append({"path": rel.as_posix(), "sha256": digest})
    aggregate = hashlib.sha256()
    for item in files:
        aggregate.update(item["path"].encode())
        aggregate.update(b"\0")
        aggregate.update(item["sha256"].encode())
        aggregate.update(b"\n")
    return {"sha256": aggregate.hexdigest(), "files": files}


def command_compile(args: argparse.Namespace) -> int:
    spec = load_document(args.source)
    policy = load_document(args.policy)
    output, resolution = compile_fern_input(spec, policy)
    write_json(args.output, output)
    if args.resolution is not None:
        write_json(args.resolution, resolution)
    return 0


def command_verify(args: argparse.Namespace) -> int:
    spec = load_document(args.source)
    policy = load_document(args.policy)
    ir = load_document(args.fern_ir)
    inventory = semantic_inventory(spec, policy, ir)
    failed = any(inventory["verification"].values())
    if args.baseline is not None:
        report = compatibility_report(
            inventory, load_document(args.baseline)
        )
        inventory["compatibility"] = report
        if report["breaking"] and not args.allow_breaking:
            failed = True
    write_json(args.inventory, inventory)
    if failed:
        raise ContractError(
            "publication verification failed; inspect "
            "verification/compatibility in inventory"
        )
    return 0


def command_digest(args: argparse.Namespace) -> int:
    report = tree_digest(args.root)
    if args.output is None:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        write_json(args.output, report)
    return 0


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description=(
            "Compile SDK surface policy to Fern input and verify publication"
        )
    )
    sub = result.add_subparsers(dest="command", required=True)

    compile_parser = sub.add_parser(
        "compile",
        help="compile surface policy to deterministic Fern OpenAPI input",
    )
    compile_parser.add_argument("--source", type=Path, required=True)
    compile_parser.add_argument("--policy", type=Path, required=True)
    compile_parser.add_argument("--output", type=Path, required=True)
    compile_parser.add_argument("--resolution", type=Path)
    compile_parser.set_defaults(func=command_compile)

    verify_parser = sub.add_parser(
        "verify", help="verify resolved policy against Fern IR"
    )
    verify_parser.add_argument("--source", type=Path, required=True)
    verify_parser.add_argument("--policy", type=Path, required=True)
    verify_parser.add_argument("--fern-ir", type=Path, required=True)
    verify_parser.add_argument("--inventory", type=Path, required=True)
    verify_parser.add_argument("--baseline", type=Path)
    verify_parser.add_argument("--allow-breaking", action="store_true")
    verify_parser.set_defaults(func=command_verify)

    digest_parser = sub.add_parser(
        "digest",
        help="hash a publishable tree excluding Fern metadata",
    )
    digest_parser.add_argument("--root", type=Path, required=True)
    digest_parser.add_argument("--output", type=Path)
    digest_parser.set_defaults(func=command_digest)
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        return args.func(args)
    except ContractError as error:
        print(f"sdk-surface: {error}")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
