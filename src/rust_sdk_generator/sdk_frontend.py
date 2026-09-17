"""Validate OpenAPI plus explicit facade policy against normalized Rust bindings.

The frontend consumes :class:`sdk_raw_ir.RawIr`. Producing that IR is exclusively
an adapter concern. This module discovers no product taxonomy, parses no generated
source, renders no Rust and performs no repository audit.
"""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
from typing import Any, Iterable

from jsonschema import Draft202012Validator
from ruamel.yaml import YAML

from .rust_symbols import SymbolProvider
from .rust_types import parse_type
from .sdk_raw_ir import (
    RawField as RustField,
    RawIr,
)
from . import sdk_ir as resolved_ir
from .sdk_ir import (
    ModelPolicy,
    RequestPolicy,
    SimpleUnionPolicy,
    StreamPolicy,
    ViewPolicy,
    model_policy,
    stream_policy,
)


class GenerationError(ValueError):
    """A source contract cannot be projected without semantic review."""


def _ref_name(schema: dict[str, Any]) -> str | None:
    ref = schema.get("$ref")
    return ref.rsplit("/", 1)[-1] if isinstance(ref, str) else None


def _success_schema(
    operation: dict[str, Any], media: str = "application/json"
) -> dict[str, Any]:
    for status, response in sorted(operation.get("responses", {}).items()):
        if str(status).startswith("2") and media in response.get("content", {}):
            return response["content"][media].get("schema", {})
    raise GenerationError(f"operation has no successful {media} response")


class OpenApiIndex:
    """Semantic source for schemas, HTTP operations and discriminators."""

    def __init__(self, document: dict[str, Any]):
        self.document = document
        self.schemas = document.get("components", {}).get("schemas", {})
        self.operations: dict[str, dict[str, Any]] = {}
        for path, path_item in document.get("paths", {}).items():
            for method in (
                "get",
                "put",
                "post",
                "delete",
                "patch",
                "head",
                "options",
            ):
                operation = path_item.get(method)
                if not operation or "operationId" not in operation:
                    continue
                combined = dict(operation)
                combined["parameters"] = path_item.get(
                    "parameters", []
                ) + operation.get("parameters", [])
                combined["x-sdk-path"] = path
                combined["x-sdk-method"] = method
                self.operations[operation["operationId"]] = combined

    @classmethod
    def load(cls, path: Path) -> "OpenApiIndex":
        return cls(YAML(typ="safe").load(path.read_text()))

    def schema(self, name: str) -> dict[str, Any]:
        try:
            return self.schemas[name]
        except KeyError as error:
            raise GenerationError(f"OpenAPI schema not found: {name}") from error

    def operation(self, operation_id: str) -> dict[str, Any]:
        try:
            return self.operations[operation_id]
        except KeyError as error:
            raise GenerationError(
                f"OpenAPI operation not found: {operation_id}"
            ) from error

    def request_schema(self, operation_id: str) -> str | None:
        request = self.operation(operation_id).get("requestBody", {})
        schema = request.get("content", {}).get("application/json", {}).get("schema", {})
        return _ref_name(schema)

    def response_schema(self, operation_id: str) -> str | None:
        return _ref_name(_success_schema(self.operation(operation_id)))

    def response_matches(self, operation_id: str, raw: str, rust: RawIr) -> bool:
        """Reconcile named and inline successful response schemas with raw Rust."""
        schema = _success_schema(self.operation(operation_id))
        referenced = _ref_name(schema)
        if referenced:
            return referenced == raw
        branches = schema.get("oneOf", []) or schema.get("anyOf", [])
        payloads = {_ref_name(branch) for branch in branches}
        if branches and None not in payloads and raw in rust.enums:
            return payloads == {variant.payload for variant in rust.variants(raw)}
        return False

    def union(self, root: str, path: Iterable[str]) -> tuple[str, dict[str, str]]:
        schema = self.schema(root)
        for segment in path:
            schema = (
                schema.get("items", {})
                if segment == "items"
                else schema.get("properties", {}).get(segment, {})
            )
            non_null = [
                part for part in schema.get("anyOf", []) if part.get("type") != "null"
            ]
            if len(non_null) == 1:
                schema = non_null[0]
        discriminator = schema.get("discriminator", {})
        mapping = {
            tag: ref.rsplit("/", 1)[-1]
            for tag, ref in discriminator.get("mapping", {}).items()
        }
        if not mapping:
            branches = schema.get("oneOf", []) or schema.get("anyOf", [])
            referenced = [_ref_name(branch) for branch in branches]
            if referenced and all(referenced):
                candidates: dict[str, dict[str, str]] = {}
                for name in referenced:
                    properties = self.schema(name).get("properties", {})
                    for property_name, property_schema in properties.items():
                        if "const" in property_schema:
                            candidates.setdefault(property_name, {})[
                                str(property_schema["const"])
                            ] = name
                complete = [
                    (name, values)
                    for name, values in candidates.items()
                    if len(values) == len(referenced)
                ]
                if len(complete) == 1:
                    property_name, mapping = complete[0]
                    discriminator = {"propertyName": property_name}
        if not mapping:
            raise GenerationError(
                f"{root}.{'.'.join(path)} is not a discriminated union"
            )
        return discriminator["propertyName"], mapping


@dataclass(frozen=True)
class ModelSpec:
    name: str
    raw: str
    config: ModelPolicy


@dataclass(frozen=True)
class OperationSpec:
    name: str
    operation_id: str
    raw_method: str
    request: str | None
    response: str | None
    request_overrides: tuple[tuple[str, bool | None], ...]
    stream: StreamPolicy | None
    request_raw: str | None = None
    empty_response: bool = False
    binary_response: bool = False


@dataclass(frozen=True)
class ResourceSpec:
    path: tuple[str, ...]
    module: str
    name: str
    operations: tuple[OperationSpec, ...]


@dataclass(frozen=True)
class SdkIr:
    client_name: str
    models: tuple[ModelSpec, ...]
    resources: tuple[ResourceSpec, ...]


def _resolved_ir(ir: SdkIr, rust: RawIr) -> resolved_ir.FacadeIr:
    """Lower the validated projection plan into the closed facade IR."""
    models = tuple(
        resolved_ir.ModelSpec(model.name, model.raw, model.config) for model in ir.models
    )
    resources = []
    for resource in ir.resources:
        operations = []
        for operation in resource.operations:
            raw = rust.operation(operation.raw_method)
            signature = resolved_ir.RawSignature(
                tuple(
                    resolved_ir.RawParameter(parameter.name, parameter.type)
                    for parameter in raw.parameters
                ),
                raw.return_type,
                raw.success_type,
            )
            if operation.request:
                request = resolved_ir.JsonRequest(
                    operation.request,
                    operation.request_raw or "",
                    operation.request_overrides,
                )
            elif raw.parameters:
                request = resolved_ir.ParametersRequest()
            else:
                request = resolved_ir.NoRequest()
            if operation.stream:
                response = resolved_ir.SseResponse(operation.stream)
            elif operation.empty_response:
                response = resolved_ir.EmptyResponse()
            elif operation.binary_response:
                response = resolved_ir.BinaryResponse()
            elif operation.response:
                model = next(
                    model for model in models if model.name == operation.response
                )
                response = resolved_ir.JsonResponse(operation.response, model.raw)
            else:
                raise GenerationError(
                    f"operation {operation.operation_id} needs a response projection"
                )
            operations.append(
                resolved_ir.OperationSpec(
                    operation.name,
                    operation.operation_id,
                    operation.raw_method,
                    signature,
                    request,
                    response,
                )
            )
        resources.append(
            resolved_ir.ResourceSpec(
                resource.path, resource.module, resource.name, tuple(operations)
            )
        )
    return resolved_ir.FacadeIr(ir.client_name, models, tuple(resources))


def _validate_keys(value: dict[str, Any], allowed: set[str], context: str) -> None:
    unknown = sorted(set(value) - allowed)
    if unknown:
        raise GenerationError(f"unknown {context} keys: {', '.join(unknown)}")


def _field_map(rust: RawIr, raw: str) -> dict[str, RustField]:
    return {field.name.removeprefix("r#"): field for field in rust.fields(raw)}


def _option(type_name: str) -> tuple[str, bool] | None:
    inner = parse_type(type_name).unary("Option")
    if inner is None:
        return None
    nullable = inner.unary("Option")
    return (nullable.spelling, True) if nullable else (inner.spelling, False)


def _request_name(resource: ResourceSpec, operation: OperationSpec) -> str:
    return (
        "".join(part.title() for part in operation.name.split("_"))
        + resource.name
        + "Request"
    )


def _validate_owned_byte_stream(raw_method: str, success_type: str) -> None:
    """Validate the Rust stream ABI required by binary/SSE facade primitives.

    The stream error type is deliberately unconstrained: it only needs an `Into`
    conversion to the configured facade error at Rust compile time.
    """
    transport = parse_type(success_type)
    if transport.constructor != "futures_util::stream::BoxStream" or len(
        transport.arguments
    ) != 2:
        raise GenerationError(f"raw stream response is not an owned BoxStream: {raw_method}")
    lifetime, event = transport.arguments
    if lifetime.spelling != "'static" or event.constructor != "Result" or len(
        event.arguments
    ) != 2:
        raise GenerationError(f"raw stream response ownership/item drift: {raw_method}")
    if event.arguments[0].spelling != "bytes::Bytes":
        raise GenerationError(f"raw stream response does not yield bytes: {raw_method}")


def build_ir(
    openapi: OpenApiIndex, rust: RawIr, manifest: dict[str, Any]
) -> resolved_ir.FacadeIr:
    schema = json.loads(
        Path(__file__).with_name("sdk-semantics.schema.json").read_text()
    )
    errors = sorted(
        Draft202012Validator(schema).iter_errors(manifest),
        key=lambda error: str(error.path),
    )
    if errors:
        error = errors[0]
        raise GenerationError(
            f"overlay /{'/'.join(map(str, error.absolute_path))}: {error.message}"
        )
    _validate_keys(
        manifest, {"schema_version", "client", "models", "resources"}, "manifest"
    )
    if manifest.get("schema_version") != 2:
        raise GenerationError("unsupported SDK semantic manifest version")
    models = []
    for name, config in manifest.get("models", {}).items():
        _validate_keys(
            config,
            {
                "raw",
                "constructor",
                "exclude",
                "adapters",
                "union",
                "simple_union",
                "type_alias",
                "map",
                "scalar_enum",
                "union_factory",
                "accessors",
                "borrowed",
            },
            f"model {name}",
        )
        raw = config.get("raw", name)
        if "union" in config:
            union = config["union"]
            _validate_keys(
                union, {"root", "path", "payload", "targets"}, f"union {name}"
            )
            _, mapping = openapi.union(union["root"], union["path"])
            for target in (raw, *union.get("targets", [])):
                variants = {variant.payload for variant in rust.variants(target)}
                missing = sorted(set(mapping.values()) - variants)
                extra = sorted(variants - set(mapping.values()))
                if missing or extra:
                    raise GenerationError(
                        f"raw union {target} branch drift: missing={missing}, extra={extra}"
                    )
        elif "simple_union" in config:
            configured = set(config["simple_union"]["variants"])
            actual = {variant.name for variant in rust.variants(raw)}
            if configured != actual:
                raise GenerationError(
                    f"raw union {raw} variant drift: missing={sorted(actual - configured)}, "
                    f"extra={sorted(configured - actual)}"
                )
        else:
            if config.get("accessors") or raw not in rust.enums:
                rust.fields(raw)
            if "constructor" in config or "union_factory" in config:
                wire_schema = openapi.object_schema(raw)
                wire_fields = set(wire_schema.get("properties", {}))
                raw_fields = {
                    field.name.removeprefix("r#") for field in rust.fields(raw)
                }
                if wire_fields != raw_fields:
                    missing = sorted(wire_fields - raw_fields)
                    extra = sorted(raw_fields - wire_fields)
                    raise GenerationError(
                        f"OpenAPI/raw field drift for {raw}: missing={missing}, extra={extra}"
                    )
                covered = set(config.get("constructor", [])) | set(
                    config.get("exclude", [])
                )
                factory = config.get("union_factory", {})
                covered |= {factory.get("field"), *factory.get("leading", [])}
                uncovered_required = set(wire_schema.get("required", [])) - covered
                if uncovered_required:
                    raise GenerationError(
                        f"required fields need constructor policy for {raw}: "
                        f"{sorted(uncovered_required)}"
                    )
        models.append(ModelSpec(name, raw, model_policy(config)))
    model_names = {model.name for model in models}
    for model in models:
        references = []
        if isinstance(model.config, RequestPolicy):
            references.extend(adapter for _, adapter in model.config.adapters)
        elif isinstance(model.config, SimpleUnionPolicy):
            references.extend(
                adapter for _, _, adapter in model.config.variants if adapter
            )
        elif isinstance(model.config, ViewPolicy):
            references.extend(
                accessor.wrapper
                for _, accessor in model.config.accessors
                if accessor.wrapper
            )
        unknown = sorted(set(references) - model_names)
        if unknown:
            raise GenerationError(
                f"model {model.name} references unknown facade models: {unknown}"
            )
    resources = []
    for module, config in manifest.get("resources", {}).items():
        _validate_keys(config, {"name", "path", "operations"}, f"resource {module}")
        operations = []
        for public_name, item in config.get("operations", {}).items():
            _validate_keys(
                item,
                {
                    "operation_id",
                    "raw_method",
                    "request",
                    "response",
                    "empty_response",
                    "binary_response",
                    "request_overrides",
                    "stream",
                },
                f"operation {module}.{public_name}",
            )
            operation_id = item["operation_id"]
            raw_method = item.get("raw_method", operation_id)
            raw_operation = rust.operation(raw_method)
            wire_operation = openapi.operation(operation_id)
            request, response = item.get("request"), item.get("response")
            empty_response = item.get("empty_response", False)
            binary_response = item.get("binary_response", False)
            success_modes = (
                int(bool(response))
                + int(bool(item.get("stream")))
                + int(empty_response)
                + int(binary_response)
            )
            if success_modes != 1:
                raise GenerationError(
                    f"operation {module}.{public_name} requires exactly one response, "
                    "stream, empty_response or binary_response projection"
                )
            for referenced in (request, response):
                if referenced and referenced not in model_names:
                    raise GenerationError(f"unknown facade model {referenced}")
            if request:
                model = next(model for model in models if model.name == request)
                if openapi.request_schema(operation_id) != model.raw:
                    raise GenerationError(f"OpenAPI request drift for {operation_id}")
                body_parameters = [
                    parameter
                    for parameter in raw_operation.parameters
                    if parameter.type == model.raw
                ]
                if len(body_parameters) != 1:
                    raise GenerationError(f"raw signature drift for {raw_method}")
                request_fields = _field_map(rust, model.raw)
                unknown_overrides = set(item.get("request_overrides", {})) - set(
                    request_fields
                )
                if unknown_overrides:
                    raise GenerationError(
                        f"unknown request overrides for {raw_method}: "
                        f"{sorted(unknown_overrides)}"
                    )
                for field, value in item.get("request_overrides", {}).items():
                    syntax = request_fields[field].syntax
                    inner = syntax.unary("Option")
                    wire_field = openapi.object_schema(model.raw).get("properties", {}).get(
                        field, {}
                    )
                    if (
                        inner is None
                        or inner.spelling != "bool"
                        or wire_field.get("type") != "boolean"
                    ):
                        raise GenerationError(
                            "request override requires optional Boolean in both contracts: "
                            f"{model.raw}.{field}"
                        )
                    if value is not None and type(value) is not bool:
                        raise GenerationError(
                            f"invalid Boolean override: {model.raw}.{field}"
                        )
            elif item.get("request_overrides"):
                raise GenerationError(
                    f"request overrides require a body projection: {raw_method}"
                )
            raw_parameters = tuple(
                parameter
                for parameter in raw_operation.parameters
                if not request or parameter.type != model.raw
            )
            wire_parameter_names = tuple(
                parameter["name"].replace("-", "_")
                for parameter in wire_operation.get("parameters", [])
            )
            raw_parameter_names = tuple(
                parameter.name.removeprefix("r#") for parameter in raw_parameters
            )
            if len(set(raw_parameter_names)) != len(raw_parameter_names):
                raise GenerationError(f"duplicate raw parameter names for {raw_method}")
            if sorted(raw_parameter_names) != sorted(wire_parameter_names):
                raise GenerationError(
                    f"OpenAPI/raw parameter drift for {raw_method}"
                )
            if empty_response:
                success = [
                    value
                    for status, value in wire_operation.get("responses", {}).items()
                    if str(status).startswith("2")
                ]
                if len(success) != 1 or success[0].get("content"):
                    raise GenerationError(
                        f"empty response drift for {operation_id}"
                    )
                if raw_operation.success_type != "()":
                    raise GenerationError(
                        f"raw empty response drift for {raw_method}"
                    )
            if binary_response:
                success = [
                    value
                    for status, value in wire_operation.get("responses", {}).items()
                    if str(status).startswith("2")
                ]
                if len(success) != 1 or len(success[0].get("content", {})) != 1:
                    raise GenerationError(
                        f"binary response drift for {operation_id}"
                    )
                payload = next(iter(success[0]["content"].values()))
                schema = payload.get("schema", {})
                if schema.get("type") != "string" or schema.get("format") != "binary":
                    raise GenerationError(
                        f"binary response drift for {operation_id}"
                    )
                _validate_owned_byte_stream(raw_method, raw_operation.success_type)
            if response and not item.get("stream"):
                model = next(model for model in models if model.name == response)
                if not openapi.response_matches(operation_id, model.raw, rust):
                    raise GenerationError(
                        f"OpenAPI response drift for {operation_id}"
                    )
                if raw_operation.success_type != model.raw:
                    raise GenerationError(f"raw response drift for {raw_method}")
            if item.get("stream"):
                stream = stream_policy(item["stream"])
                if not request:
                    raise GenerationError(
                        f"stream requires a request projection: {raw_method}"
                    )
                wire_schema = _success_schema(wire_operation, "text/event-stream")
                wire_item = _ref_name(wire_schema)
                if wire_item and wire_item != stream.item:
                    envelope = openapi.schema(wire_item)
                    wire_item = _ref_name(
                        envelope.get("properties", {}).get("data", {})
                    )
                    if "data" not in envelope.get("required", []):
                        raise GenerationError(
                            f"stream envelope has no required data: {raw_method}"
                        )
                if wire_item != stream.item:
                    raise GenerationError(
                        f"stream payload drift for {raw_method}: "
                        f"{wire_item} != {stream.item}"
                    )
                rust.fields(stream.item)
                wrapper = next(
                    (model for model in models if model.name == stream.wrapper), None
                )
                if (
                    wrapper is None
                    or wrapper.raw != stream.item
                    or not isinstance(wrapper.config, ViewPolicy)
                    or wrapper.config.borrowed
                ):
                    raise GenerationError(
                        f"stream wrapper must own the configured item: {stream.wrapper}"
                    )
                _validate_owned_byte_stream(raw_method, raw_operation.success_type)
            request_raw = (
                next(model.raw for model in models if model.name == request)
                if request
                else None
            )
            operations.append(
                OperationSpec(
                    public_name,
                    operation_id,
                    raw_method,
                    request,
                    response,
                    tuple(item.get("request_overrides", {}).items()),
                    stream_policy(item.get("stream")),
                    request_raw,
                    empty_response,
                    binary_response,
                )
            )
        resource_path = tuple(config.get("path", (module,)))
        if not resource_path:
            raise GenerationError(f"resource {module} has an empty path")
        resources.append(
            ResourceSpec(resource_path, module, config["name"], tuple(operations))
        )
    plan = SdkIr(
        manifest.get("client", {}).get("name", "Client"),
        tuple(models),
        tuple(resources),
    )
    _validate_symbols(plan, rust)
    return _resolved_ir(plan, rust)


def _validate_symbols(ir: SdkIr, rust: RawIr) -> None:
    """Validate compiler-owned public symbols only.

    Consumer runtime names are validated against the resolved IR later, when the
    explicit :class:`RustFacadeRuntime` is available.
    """
    symbols = SymbolProvider()
    try:
        symbols.claim(ir.client_name, "sdk", "client")
        for reserved in ("new", "raw", "with_base_url"):
            symbols.claim(reserved, ir.client_name, "client runtime")
        for model in ir.models:
            symbols.claim(model.name, "sdk", f"model {model.raw}", "facade_types")
            if model.name in rust.structs or model.name in rust.enums:
                raise ValueError(
                    f"facade model {model.name} shadows imported raw type; "
                    "choose an explicit semantic name"
                )
        for reserved in ("facade_types", "mod"):
            symbols.claim(
                reserved if reserved != "mod" else "mod_file",
                "modules",
                "compiler",
            )
        by_path = {resource.path: resource for resource in ir.resources}
        for resource in ir.resources:
            if resource.module == "mod":
                raise ValueError("resource module mod is reserved")
            symbols.claim(resource.module, "modules", "resource")
            symbols.claim(resource.name, "sdk", "resource", resource.module)
            symbols.claim("new", resource.name, "resource constructor")
            if len(resource.path) == 1:
                symbols.claim(resource.path[0], ir.client_name, "resource accessor")
            else:
                parent = by_path.get(resource.path[:-1])
                if parent is None:
                    raise ValueError(
                        f"resource {'.'.join(resource.path)} has no parent"
                    )
                symbols.claim(
                    resource.path[-1], parent.name, "child resource accessor"
                )
            for operation in resource.operations:
                symbols.claim(
                    operation.name, resource.name, operation.operation_id
                )
                parameters = rust.operation(operation.raw_method).parameters
                if (
                    not operation.request
                    and parameters
                    and any(_option(parameter.type) for parameter in parameters)
                ):
                    symbols.claim(
                        _request_name(resource, operation),
                        f"module:{resource.module}",
                        operation.operation_id,
                    )
                    if all(_option(parameter.type) for parameter in parameters):
                        symbols.claim(
                            operation.name + "_with",
                            resource.name,
                            operation.operation_id,
                        )
                if operation.stream:
                    symbols.claim(
                        operation.stream.type,
                        "sdk",
                        operation.operation_id,
                        "facade_types",
                    )
    except ValueError as error:
        raise GenerationError(str(error)) from error
