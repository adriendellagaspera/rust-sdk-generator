"""Resolve model rendering semantics from validated wire/raw contracts.

This is the last source-aware stage for facade models. The Rust renderer receives
only immutable values from :mod:`sdk_ir` and never inspects OpenAPI or raw source.
"""

from dataclasses import replace

from .rust_symbols import field_identifier
from .rust_types import RustType, parse_type
from .sdk_ir import (
    AccessorKind,
    AliasModelSpec,
    ArgumentKind,
    ArgumentSpec,
    CollectIntoValue,
    ConstructorSpec,
    EnumValue,
    FactorySpec,
    FacadeIr,
    IntoModelValue,
    IntoStringValue,
    LiteralValue,
    MapIntoValue,
    MapModelSpec,
    MapPolicy,
    ModelSpec,
    RequestPolicy,
    ResolvedAccessor,
    ScalarEnumModelSpec,
    ScalarEnumPolicy,
    SetterSpec,
    SimpleUnionBranchSpec,
    SimpleUnionModelSpec,
    SimpleUnionPolicy,
    SomeValue,
    StructFieldValue,
    StructValue,
    TypeAliasPolicy,
    UnionBranchSpec,
    UnionModelSpec,
    UnionPolicy,
    UnionTargetSpec,
    VariableValue,
    ViewModelSpec,
    ViewPolicy,
    WrapperModelSpec,
)


class ModelLoweringError(ValueError):
    """A validated model contract cannot be represented by resolved facade IR."""


def _option(type_name: str) -> tuple[str, int] | None:
    inner = parse_type(type_name).unary("Option")
    if inner is None:
        return None
    nullable = inner.unary("Option")
    return (nullable.spelling, 2) if nullable else (inner.spelling, 1)


def _wrap(type_name: str, value):
    optional = _option(type_name)
    return SomeValue(value, optional[1]) if optional else value


def _argument(name: str, type_name: str) -> tuple[ArgumentSpec, object]:
    public_name = field_identifier(name)
    if type_name == "String":
        return (
            ArgumentSpec(public_name, ArgumentKind.INTO_STRING, "String"),
            IntoStringValue(public_name),
        )
    return (
        ArgumentSpec(public_name, ArgumentKind.EXACT, type_name),
        VariableValue(public_name),
    )


def _constructor_argument(
    name: str, field, adapter: str | None
) -> tuple[ArgumentSpec, object]:
    public_name = field_identifier(name)
    if adapter:
        if parse_type(field.type).unary("Vec") is not None:
            return (
                ArgumentSpec(public_name, ArgumentKind.INTO_ITER_MODEL, adapter),
                CollectIntoValue(public_name),
            )
        return (
            ArgumentSpec(public_name, ArgumentKind.INTO_MODEL, adapter),
            IntoModelValue(public_name, adapter),
        )
    optional = _option(field.type)
    effective = optional[0] if optional else field.type
    argument, value = _argument(name, effective)
    return argument, _wrap(field.type, value)


def _field_map(raw_index, raw: str):
    return {field.name.removeprefix("r#"): field for field in raw_index.fields(raw)}


def _struct_value(raw: str, fields: dict, values: dict[str, object]) -> StructValue:
    assignments = []
    for name, field in fields.items():
        if name in values:
            value = values[name]
            shorthand = (
                isinstance(value, VariableValue)
                and value.name == field.name
                and not field.name.startswith("r#")
            )
            assignments.append(StructFieldValue(field.name, value, shorthand))
        elif _option(field.type):
            assignments.append(StructFieldValue(field.name, LiteralValue("None")))
        else:
            raise ModelLoweringError(
                f"required raw field {raw}.{name} has no value"
            )
    return StructValue(raw, tuple(assignments))


def _factory_value(
    type_name: str, argument: str, raw_index
) -> tuple[ArgumentSpec, object]:
    public_name = field_identifier(argument)
    optional = _option(type_name)
    core = optional[0] if optional else type_name
    if core == "String":
        spec = ArgumentSpec(public_name, ArgumentKind.INTO_STRING, "String")
        value = IntoStringValue(public_name)
    elif core in raw_index.enums:
        string = next(
            (
                variant
                for variant in raw_index.variants(core)
                if variant.payload == "String"
            ),
            None,
        )
        if string is None:
            spec = ArgumentSpec(public_name, ArgumentKind.EXACT, core)
            value = VariableValue(public_name)
        else:
            spec = ArgumentSpec(public_name, ArgumentKind.INTO_STRING, "String")
            value = EnumValue(core, string.name, IntoStringValue(public_name))
    else:
        spec = ArgumentSpec(public_name, ArgumentKind.EXACT, core)
        value = VariableValue(public_name)
    return spec, _wrap(type_name, value)


def _resolve_wrapper(model: ModelSpec, openapi, raw_index) -> WrapperModelSpec:
    assert isinstance(model.config, RequestPolicy)
    fields = _field_map(raw_index, model.raw)
    constructor = None
    if model.config.constructor or model.config.factory is None:
        arguments, values = [], {}
        adapters = dict(model.config.adapters)
        for name in model.config.constructor:
            if name not in fields:
                raise ModelLoweringError(
                    f"constructor field {model.raw}.{name} not found"
                )
            argument, value = _constructor_argument(
                name, fields[name], adapters.get(name)
            )
            arguments.append(argument)
            values[name] = value
        constructor = ConstructorSpec(
            tuple(arguments), _struct_value(model.raw, fields, values)
        )

    factories = []
    if model.config.factory is not None:
        config = model.config.factory
        field_name = config.field
        union_type = _option(fields[field_name].type)
        raw_union = union_type[0] if union_type else fields[field_name].type
        discriminator, mapping = openapi.union(model.raw, [field_name])
        raw_variants = {
            variant.payload: variant.name for variant in raw_index.variants(raw_union)
        }
        for tag, payload in sorted(mapping.items()):
            payload_schema = openapi.schema(payload)
            candidates = [
                name
                for name in payload_schema.get("required", [])
                if name != discriminator
            ]
            if len(candidates) != 1:
                raise ModelLoweringError(
                    f"factory branch {payload} needs one non-discriminator input, "
                    f"got {candidates}"
                )
            input_name = candidates[0]
            payload_fields = _field_map(raw_index, payload)
            if input_name not in payload_fields:
                raise ModelLoweringError(
                    f"raw field {payload}.{input_name} not found"
                )
            arguments, outer_values = [], {}
            for leading in config.leading:
                argument, value = _factory_value(
                    fields[leading].type, leading, raw_index
                )
                arguments.append(argument)
                outer_values[leading] = value
            argument, value = _factory_value(
                payload_fields[input_name].type, input_name, raw_index
            )
            arguments.append(argument)
            payload_value = _struct_value(
                payload, payload_fields, {input_name: value}
            )
            union_value = EnumValue(raw_union, raw_variants[payload], payload_value)
            outer_values[field_name] = _wrap(fields[field_name].type, union_value)
            public_name = dict(config.rename).get(tag, tag.replace("-", "_"))
            factories.append(
                FactorySpec(
                    public_name,
                    tuple(arguments),
                    _struct_value(model.raw, fields, outer_values),
                )
            )

    setters = []
    excluded = set(model.config.exclude) | set(model.config.constructor)
    adapters = dict(model.config.adapters)
    for field in raw_index.fields(model.raw):
        name = field.name.removeprefix("r#")
        if name in excluded:
            continue
        optional = _option(field.type)
        if optional is None:
            raise ModelLoweringError(
                f"new required field {model.raw}.{name}; constructor policy needs semantic review"
            )
        inner, depth = optional
        adapter = adapters.get(name)
        if adapter:
            class InnerField:
                type = inner

            argument, value = _constructor_argument(name, InnerField(), adapter)
        else:
            argument, value = _argument(name, inner)
        setters.append(
            SetterSpec(
                field_identifier(name),
                field.name,
                argument,
                SomeValue(value, depth),
                f"{name}_null" if depth == 2 else None,
            )
        )
    return WrapperModelSpec(
        constructor,
        tuple(factories),
        tuple(setters),
        constructor is not None
        and not model.config.constructor
        and model.config.factory is None,
    )


def _public_variant(tag: str) -> str:
    return "".join(
        part[:1].upper() + part[1:]
        for part in tag.replace("-", "_").split("_")
        if part
    )


def _string_payload(
    raw_payload: str, payload_field: str, raw_index
) -> StructValue:
    assignments = []
    for field in raw_index.fields(raw_payload):
        name = field.name.removeprefix("r#")
        if name == payload_field:
            optional = _option(field.type)
            core = optional[0] if optional else field.type
            if core == "String":
                value = VariableValue("content")
            else:
                string_variant = next(
                    (
                        variant
                        for variant in raw_index.variants(core)
                        if variant.payload == "String"
                    ),
                    None,
                )
                if string_variant is None:
                    raise ModelLoweringError(
                        f"{raw_payload}.{payload_field} has no String branch"
                    )
                value = EnumValue(
                    core, string_variant.name, VariableValue("content")
                )
            assignments.append(
                StructFieldValue(field.name, _wrap(field.type, value))
            )
        elif _option(field.type):
            assignments.append(StructFieldValue(field.name, LiteralValue("None")))
        else:
            raise ModelLoweringError(f"{raw_payload} also requires {name}")
    return StructValue(raw_payload, tuple(assignments))


def _resolve_union(model: ModelSpec, openapi, raw_index) -> UnionModelSpec:
    assert isinstance(model.config, UnionPolicy)
    _, mapping = openapi.union(model.config.root, model.config.path)
    branches = []
    for tag, raw_payload in sorted(mapping.items()):
        public = _public_variant(tag)
        try:
            raw_value = _string_payload(
                raw_payload, model.config.payload, raw_index
            )
        except ModelLoweringError:
            argument = ArgumentSpec("value", ArgumentKind.EXACT, raw_payload)
            raw_value = VariableValue("value")
            public_type = raw_payload
        else:
            argument = ArgumentSpec(
                "content", ArgumentKind.INTO_STRING, "String"
            )
            public_type = "String"
        branches.append(
            UnionBranchSpec(
                public,
                tag.replace("-", "_"),
                public_type,
                argument,
                raw_payload,
                raw_value,
            )
        )

    targets = []
    for target in (model.raw, *model.config.targets):
        raw_variants = {
            variant.payload: variant.name for variant in raw_index.variants(target)
        }
        targets.append(
            UnionTargetSpec(
                target,
                tuple(
                    (payload, raw_variants[payload])
                    for payload in mapping.values()
                ),
            )
        )
    return UnionModelSpec(tuple(branches), tuple(targets))


def _expand_alias(
    syntax: RustType, raw_index, seen: tuple[str, ...] = ()
) -> RustType:
    if syntax.spelling not in raw_index.aliases:
        return syntax
    if syntax.spelling in seen:
        raise ModelLoweringError(f"recursive raw type alias: {syntax.spelling}")
    return _expand_alias(
        raw_index.aliases[syntax.spelling], raw_index, (*seen, syntax.spelling)
    )


def _adapted_public_type(
    syntax: RustType, adapter: str, raw_index
) -> tuple[str, int]:
    syntax = _expand_alias(syntax, raw_index)
    if syntax.constructor == "Vec":
        inner, depth = _adapted_public_type(
            syntax.arguments[0], adapter, raw_index
        )
        return f"Vec<{inner}>", depth + 1
    return adapter, 0


def _resolve_simple_union(
    model: ModelSpec, raw_index
) -> SimpleUnionModelSpec:
    assert isinstance(model.config, SimpleUnionPolicy)
    raw_variants = {
        variant.name: variant for variant in raw_index.variants(model.raw)
    }
    branches = []
    for raw_name, public_name, adapter in model.config.variants:
        payload = raw_variants[raw_name].payload
        if payload is None:
            raise ModelLoweringError(
                f"simple union {model.raw}::{raw_name} has no payload"
            )
        raw_syntax = _expand_alias(parse_type(payload), raw_index)
        if adapter:
            public_type, depth = _adapted_public_type(
                raw_syntax, adapter, raw_index
            )
            adapt_depth = depth
        else:
            public_type = raw_index.qualified_type(raw_syntax.spelling)
            adapt_depth = None
        branches.append(
            SimpleUnionBranchSpec(
                raw_name, public_name, public_type, adapt_depth
            )
        )
    return SimpleUnionModelSpec(tuple(branches), model.config.bidirectional)


def _generic_inner(type_name: str, constructor: str) -> str:
    inner = parse_type(type_name).unary(constructor)
    if inner is None:
        raise ModelLoweringError(f"expected {constructor}, got {type_name}")
    return inner.spelling


def _accessor_type(raw: str, path: tuple[str, ...], raw_index) -> str:
    current = raw
    for segment in path:
        if segment == "first":
            current = _generic_inner(current, "Vec")
        elif segment == "optional":
            current = _generic_inner(current, "Option")
        else:
            fields = _field_map(raw_index, current)
            if segment not in fields:
                raise ModelLoweringError(
                    f"accessor path field {current}.{segment} not found"
                )
            current = fields[segment].type
    return current


def _resolve_view(model: ModelSpec, raw_index) -> ViewModelSpec:
    assert isinstance(model.config, ViewPolicy)
    accessors = []
    for name, config in model.config.accessors:
        result_type = _accessor_type(model.raw, config.path, raw_index)
        if config.kind == AccessorKind.COPY:
            return_type = result_type
            accessors.append(
                ResolvedAccessor(name, config.kind, config.path, return_type)
            )
        elif config.kind == AccessorKind.REF:
            public_type = "str" if result_type == "String" else result_type
            accessors.append(
                ResolvedAccessor(
                    name, config.kind, config.path, f"&{public_type}"
                )
            )
        elif config.kind in {
            AccessorKind.OPTIONAL_COPY,
            AccessorKind.OPTIONAL_REF,
        }:
            inner = parse_type(result_type).unary("Option")
            if inner is None:
                raise ModelLoweringError(
                    f"accessor {name} expected Option, got {result_type}"
                )
            if config.kind == AccessorKind.OPTIONAL_COPY:
                return_type = f"Option<{inner.spelling}>"
            else:
                vector = inner.unary("Vec")
                if inner.spelling == "String":
                    public_type = "str"
                elif vector:
                    public_type = f"[{vector.spelling}]"
                else:
                    public_type = inner.spelling
                return_type = f"Option<&{public_type}>"
            accessors.append(
                ResolvedAccessor(name, config.kind, config.path, return_type)
            )
        elif config.kind == AccessorKind.ITER:
            _generic_inner(result_type, "Vec")
            if config.wrapper is None:
                raise ModelLoweringError(
                    f"iter accessor {name} requires a wrapper"
                )
            accessors.append(
                ResolvedAccessor(
                    name,
                    config.kind,
                    config.path,
                    f"impl ExactSizeIterator<Item = {config.wrapper}<'_>>",
                    wrapper=config.wrapper,
                )
            )
        elif config.kind == AccessorKind.FIRST_STRING_VARIANT:
            string_variant = next(
                (
                    variant
                    for variant in raw_index.variants(result_type)
                    if variant.payload == "String"
                ),
                None,
            )
            if string_variant is None:
                raise ModelLoweringError(
                    f"accessor {name} target {result_type} has no String branch"
                )
            accessors.append(
                ResolvedAccessor(
                    name,
                    config.kind,
                    config.path,
                    "Option<&str>",
                    enum_type=result_type,
                    enum_variant=string_variant.name,
                )
            )
        else:
            raise ModelLoweringError(
                f"unsupported accessor kind: {config.kind}"
            )
    return ViewModelSpec(model.config.borrowed, tuple(accessors))


def _public_alias_type(
    syntax: RustType, raw_index, seen: tuple[str, ...] = ()
) -> str:
    if syntax.spelling in raw_index.aliases:
        if syntax.spelling in seen:
            raise ModelLoweringError(
                f"recursive raw type alias: {syntax.spelling}"
            )
        return _public_alias_type(
            raw_index.aliases[syntax.spelling],
            raw_index,
            (*seen, syntax.spelling),
        )
    if syntax.spelling in raw_index.symbol_paths:
        raise ModelLoweringError(
            f"public type alias references generated symbol: {syntax.spelling}"
        )
    if syntax.kind == "generic_type":
        arguments = ", ".join(
            _public_alias_type(argument, raw_index, seen)
            for argument in syntax.arguments
        )
        return f"{syntax.constructor}<{arguments}>"
    return syntax.spelling


def _unwrap_nullable_schema(schema):
    branches = schema.get("anyOf", [])
    non_null = [branch for branch in branches if branch.get("type") != "null"]
    return (
        non_null[0]
        if len(non_null) == 1 and len(non_null) != len(branches)
        else schema
    )


def _schema_at(openapi, root: str, path: tuple[str, ...]):
    schema = openapi.schema(root)
    for segment in path:
        schema = _unwrap_nullable_schema(schema)
        schema = (
            schema.get("items", {})
            if segment == "items"
            else schema.get("properties", {}).get(segment, {})
        )
    return _unwrap_nullable_schema(schema)


def _resolve_map(model: ModelSpec, openapi, raw_index) -> MapModelSpec:
    assert isinstance(model.config, MapPolicy)
    schema = _schema_at(openapi, model.config.root, model.config.path)
    additional = schema.get("additionalProperties")
    if schema.get("type") != "object" or not additional:
        raise ModelLoweringError(
            f"map policy {model.name} does not resolve to additionalProperties"
        )
    fields = raw_index.fields(model.raw)
    if (
        len(fields) != 1
        or fields[0].name.removeprefix("r#") != "additional_properties"
    ):
        raise ModelLoweringError(
            f"raw map wrapper {model.raw} must contain only additional_properties"
        )
    field = fields[0]
    mapping = parse_type(field.type)
    if (
        mapping.constructor != "std::collections::BTreeMap"
        or len(mapping.arguments) != 2
    ):
        raise ModelLoweringError(
            f"raw map wrapper {model.raw}.{field.name} is not a BTreeMap"
        )
    key, value = mapping.arguments
    if key.spelling != "String":
        raise ModelLoweringError(
            f"raw map wrapper {model.raw} has non-String keys"
        )
    effective = _expand_alias(value, raw_index)
    if effective.spelling != "serde_json::Value":
        if additional is True or not isinstance(additional, dict):
            raise ModelLoweringError(
                f"raw map value drift for {model.raw}: {effective.spelling}"
            )
        additional = _unwrap_nullable_schema(additional)
        expected = {
            "string": "String",
            "integer": "i64",
            "number": "f64",
            "boolean": "bool",
        }.get(additional.get("type"))
        if expected != effective.spelling:
            raise ModelLoweringError(
                f"raw map value drift for {model.raw}: "
                f"{effective.spelling} != {expected}"
            )
    return MapModelSpec(_public_alias_type(mapping, raw_index), field.name)


def _resolve_scalar_enum(
    model: ModelSpec, openapi, raw_index
) -> ScalarEnumModelSpec:
    assert isinstance(model.config, ScalarEnumPolicy)
    schema = _schema_at(openapi, model.config.root, model.config.path)
    values = schema.get("enum")
    if (
        schema.get("type") != "string"
        or not isinstance(values, list)
        or not values
        or not all(isinstance(value, str) for value in values)
    ):
        raise ModelLoweringError(
            f"scalar enum policy {model.name} does not resolve to a string enum"
        )
    variants = raw_index.variants(model.raw)
    if not variants or any(
        variant.payload is not None or variant.wire_name is None
        for variant in variants
    ):
        raise ModelLoweringError(
            f"raw scalar enum {model.raw} requires unit variants with serde rename provenance"
        )
    by_wire = {variant.wire_name: variant.name for variant in variants}
    if len(by_wire) != len(variants) or set(by_wire) != set(values):
        raise ModelLoweringError(
            f"raw scalar enum {model.raw} wire drift: "
            f"expected={sorted(values)}, actual={sorted(by_wire)}"
        )
    return ScalarEnumModelSpec(tuple(by_wire[value] for value in values))


def _resolve_model(model: ModelSpec, openapi, raw_index):
    if isinstance(model.config, UnionPolicy):
        return _resolve_union(model, openapi, raw_index)
    if isinstance(model.config, SimpleUnionPolicy):
        return _resolve_simple_union(model, raw_index)
    if isinstance(model.config, TypeAliasPolicy):
        return AliasModelSpec(
            _public_alias_type(
                raw_index.aliases[model.raw], raw_index, (model.raw,)
            )
        )
    if isinstance(model.config, MapPolicy):
        return _resolve_map(model, openapi, raw_index)
    if isinstance(model.config, ScalarEnumPolicy):
        return _resolve_scalar_enum(model, openapi, raw_index)
    if isinstance(model.config, ViewPolicy):
        return _resolve_view(model, raw_index)
    return _resolve_wrapper(model, openapi, raw_index)


def resolve_models(ir: FacadeIr, openapi, raw_index) -> FacadeIr:
    """Return an equivalent IR whose model rendering inputs are complete."""
    return replace(
        ir,
        models=tuple(
            replace(model, render=_resolve_model(model, openapi, raw_index))
            for model in ir.models
        ),
    )
