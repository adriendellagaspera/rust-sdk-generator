"""Immutable policy and facade IR for the semantic SDK compiler.

Only the frontend handles JSON dictionaries. Policy objects capture deliberate
product semantics; resolved facade objects capture the complete public/raw shape
selected during lowering before Rust emission.
"""
from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum


class AccessorKind(StrEnum):
    COPY = "copy"
    REF = "ref"
    OPTIONAL_COPY = "optional_copy"
    OPTIONAL_REF = "optional_ref"
    ITER = "iter"
    FIRST_STRING_VARIANT = "first_string_variant"


@dataclass(frozen=True)
class Accessor:
    kind: AccessorKind
    path: tuple[str, ...]
    wrapper: str | None


@dataclass(frozen=True)
class UnionPolicy:
    root: str
    path: tuple[str, ...]
    payload: str
    targets: tuple[str, ...]


@dataclass(frozen=True)
class SimpleUnionPolicy:
    variants: tuple[tuple[str, str, str | None], ...]
    bidirectional: bool


@dataclass(frozen=True)
class UnionFactory:
    field: str
    leading: tuple[str, ...]
    rename: tuple[tuple[str, str], ...]


@dataclass(frozen=True)
class TypeAliasPolicy:
    pass


@dataclass(frozen=True)
class MapPolicy:
    root: str
    path: tuple[str, ...]


@dataclass(frozen=True)
class ScalarEnumPolicy:
    root: str
    path: tuple[str, ...]


@dataclass(frozen=True)
class RequestPolicy:
    constructor: tuple[str, ...]
    exclude: tuple[str, ...]
    adapters: tuple[tuple[str, str], ...]
    factory: UnionFactory | None


@dataclass(frozen=True)
class ViewPolicy:
    borrowed: bool
    accessors: tuple[tuple[str, Accessor], ...]


ModelPolicy = (UnionPolicy | SimpleUnionPolicy | TypeAliasPolicy | MapPolicy | ScalarEnumPolicy |
               RequestPolicy | ViewPolicy)


@dataclass(frozen=True)
class StreamPolicy:
    item: str
    wrapper: str
    type: str


@dataclass(frozen=True)
class RawParameter:
    name: str
    type: str


@dataclass(frozen=True)
class RawSignature:
    parameters: tuple[RawParameter, ...]
    return_type: str
    success_type: str


@dataclass(frozen=True)
class NoRequest:
    """Operation with no request body or explicit HTTP parameters."""


@dataclass(frozen=True)
class ParametersRequest:
    """Operation whose public request is composed only of HTTP parameters."""


@dataclass(frozen=True)
class JsonRequest:
    model: str
    raw: str
    overrides: tuple[tuple[str, bool | None], ...]


RequestProjection = NoRequest | ParametersRequest | JsonRequest


@dataclass(frozen=True)
class ParameterField:
    name: str
    type: str
    constructor_argument: str | None
    constructor_value: str | None
    setter_argument: str | None
    setter_value: str | None


@dataclass(frozen=True)
class ParameterRequestSpec:
    name: str
    fields: tuple[ParameterField, ...]


@dataclass(frozen=True)
class OperationCall:
    arguments: str
    raw_arguments: str
    default_raw_arguments: str | None


@dataclass(frozen=True)
class JsonResponse:
    model: str
    raw: str


@dataclass(frozen=True)
class EmptyResponse:
    """Successful response with no response body."""


@dataclass(frozen=True)
class BinaryResponse:
    """Successful response exposed as the SDK binary stream primitive."""


@dataclass(frozen=True)
class SseResponse:
    stream: StreamPolicy


ResponseProjection = JsonResponse | EmptyResponse | BinaryResponse | SseResponse


class ArgumentKind(StrEnum):
    EXACT = "exact"
    INTO_STRING = "into_string"
    INTO_MODEL = "into_model"
    INTO_ITER_MODEL = "into_iter_model"


@dataclass(frozen=True)
class ArgumentSpec:
    name: str
    kind: ArgumentKind
    type: str


@dataclass(frozen=True)
class VariableValue:
    name: str


@dataclass(frozen=True)
class IntoStringValue:
    name: str


@dataclass(frozen=True)
class IntoModelValue:
    name: str
    adapter: str


@dataclass(frozen=True)
class CollectIntoValue:
    name: str


@dataclass(frozen=True)
class MapIntoValue:
    name: str
    depth: int


@dataclass(frozen=True)
class SomeValue:
    value: ValueSpec
    depth: int = 1


@dataclass(frozen=True)
class EnumValue:
    type: str
    variant: str
    value: ValueSpec


@dataclass(frozen=True)
class StructFieldValue:
    name: str
    value: ValueSpec
    shorthand: bool = False


@dataclass(frozen=True)
class StructValue:
    type: str
    fields: tuple[StructFieldValue, ...]


@dataclass(frozen=True)
class LiteralValue:
    value: str


ValueSpec = (VariableValue | IntoStringValue | IntoModelValue | CollectIntoValue |
             MapIntoValue | SomeValue | EnumValue | StructValue | LiteralValue)


@dataclass(frozen=True)
class ConstructorSpec:
    arguments: tuple[ArgumentSpec, ...]
    value: StructValue


@dataclass(frozen=True)
class FactorySpec:
    name: str
    arguments: tuple[ArgumentSpec, ...]
    value: StructValue


@dataclass(frozen=True)
class SetterSpec:
    name: str
    raw_field: str
    argument: ArgumentSpec
    value: ValueSpec
    null_name: str | None


@dataclass(frozen=True)
class WrapperModelSpec:
    constructor: ConstructorSpec | None
    factories: tuple[FactorySpec, ...]
    setters: tuple[SetterSpec, ...]
    default: bool


@dataclass(frozen=True)
class UnionBranchSpec:
    public_name: str
    constructor_name: str
    public_type: str
    argument: ArgumentSpec
    raw_payload: str
    raw_value: ValueSpec


@dataclass(frozen=True)
class UnionTargetSpec:
    raw: str
    variants: tuple[tuple[str, str], ...]


@dataclass(frozen=True)
class UnionModelSpec:
    branches: tuple[UnionBranchSpec, ...]
    targets: tuple[UnionTargetSpec, ...]


@dataclass(frozen=True)
class SimpleUnionBranchSpec:
    raw_name: str
    public_name: str
    public_type: str
    adapt_depth: int | None


@dataclass(frozen=True)
class SimpleUnionModelSpec:
    branches: tuple[SimpleUnionBranchSpec, ...]
    bidirectional: bool


@dataclass(frozen=True)
class ResolvedAccessor:
    name: str
    kind: AccessorKind
    path: tuple[str, ...]
    return_type: str
    wrapper: str | None = None
    enum_type: str | None = None
    enum_variant: str | None = None


@dataclass(frozen=True)
class ViewModelSpec:
    borrowed: bool
    accessors: tuple[ResolvedAccessor, ...]


@dataclass(frozen=True)
class AliasModelSpec:
    public_type: str


@dataclass(frozen=True)
class MapModelSpec:
    public_type: str
    raw_field: str


@dataclass(frozen=True)
class ScalarEnumModelSpec:
    variants: tuple[str, ...]


ModelRenderSpec = (WrapperModelSpec | UnionModelSpec | SimpleUnionModelSpec |
                   ViewModelSpec | AliasModelSpec | MapModelSpec | ScalarEnumModelSpec)


@dataclass(frozen=True)
class ModelSpec:
    name: str
    raw: str
    config: ModelPolicy
    render: ModelRenderSpec | None = None


@dataclass(frozen=True)
class OperationSpec:
    name: str
    operation_id: str
    raw_method: str
    raw_signature: RawSignature
    request_projection: RequestProjection
    response_projection: ResponseProjection
    call: OperationCall | None = None
    parameter_request: ParameterRequestSpec | None = None

    @property
    def request(self) -> str | None:
        return self.request_projection.model if isinstance(self.request_projection, JsonRequest) else None

    @property
    def request_raw(self) -> str | None:
        return self.request_projection.raw if isinstance(self.request_projection, JsonRequest) else None

    @property
    def request_overrides(self) -> tuple[tuple[str, bool | None], ...]:
        return self.request_projection.overrides if isinstance(self.request_projection, JsonRequest) else ()

    @property
    def response(self) -> str | None:
        return self.response_projection.model if isinstance(self.response_projection, JsonResponse) else None

    @property
    def stream(self) -> StreamPolicy | None:
        return self.response_projection.stream if isinstance(self.response_projection, SseResponse) else None

    @property
    def empty_response(self) -> bool:
        return isinstance(self.response_projection, EmptyResponse)

    @property
    def binary_response(self) -> bool:
        return isinstance(self.response_projection, BinaryResponse)


@dataclass(frozen=True)
class ResourceSpec:
    path: tuple[str, ...]
    module: str
    name: str
    operations: tuple[OperationSpec, ...]


@dataclass(frozen=True)
class FacadeIr:
    client_name: str
    models: tuple[ModelSpec, ...]
    resources: tuple[ResourceSpec, ...]


def model_policy(config: dict) -> ModelPolicy:
    if "union" in config:
        union = config["union"]
        return UnionPolicy(union["root"], tuple(union["path"]), union["payload"],
                           tuple(union.get("targets", ())))
    if "simple_union" in config:
        return SimpleUnionPolicy(tuple(
            (raw, value if isinstance(value, str) else value["name"],
             None if isinstance(value, str) else value.get("adapter"))
            for raw, value in config["simple_union"]["variants"].items()
        ), config["simple_union"].get("bidirectional", False))
    if config.get("type_alias"):
        return TypeAliasPolicy()
    if "map" in config:
        mapping = config["map"]
        return MapPolicy(mapping["root"], tuple(mapping.get("path", ())))
    if "scalar_enum" in config:
        enum = config["scalar_enum"]
        return ScalarEnumPolicy(enum["root"], tuple(enum.get("path", ())))
    if "accessors" in config:
        return ViewPolicy(config.get("borrowed", True), tuple(
            (name, Accessor(AccessorKind(value["kind"]), tuple(value["path"]), value.get("wrapper")))
            for name, value in config["accessors"].items()))
    factory = config.get("union_factory")
    return RequestPolicy(tuple(config.get("constructor", ())), tuple(config.get("exclude", ())),
                         tuple(config.get("adapters", {}).items()),
                         UnionFactory(factory["field"], tuple(factory.get("leading", ())),
                                      tuple(factory.get("rename", {}).items())) if factory else None)


def stream_policy(config: dict | None) -> StreamPolicy | None:
    return StreamPolicy(config["item"], config.get("wrapper", config["item"]), config["type"]) if config else None
