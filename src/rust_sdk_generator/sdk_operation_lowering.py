"""Resolve operation call shapes from validated facade and raw IR.

All raw-binding inspection needed by operation/resource emission stops here.
The renderer receives only immutable values from :mod:`sdk_ir`.
"""

from dataclasses import replace

from .rust_symbols import field_identifier
from .rust_types import parse_type
from .sdk_ir import (
    FacadeIr,
    JsonRequest,
    OperationCall,
    OperationSpec,
    ParameterField,
    ParameterRequestSpec,
    ResourceSpec,
)


class OperationLoweringError(ValueError):
    """A validated raw signature cannot be represented by the facade IR."""


def _option(type_name: str) -> str | None:
    inner = parse_type(type_name).unary("Option")
    return inner.spelling if inner is not None else None


def _argument(name: str, type_name: str) -> tuple[str, str]:
    name = field_identifier(name)
    return (
        (f"{name}: impl Into<String>", f"{name}.into()")
        if type_name == "String"
        else (f"{name}: {type_name}", name)
    )


def _owned_parameter(type_name: str) -> tuple[str, bool]:
    optional = _option(type_name)
    inner = optional if optional is not None else type_name
    if inner in {"impl AsRef<str>", "&str"}:
        return ("Option<String>" if optional is not None else "String"), True
    if "impl " in inner or "&" in inner:
        raise OperationLoweringError(
            f"unsupported owned parameter projection: {type_name}"
        )
    return type_name, False


def _direct_parameter(parameter, raw_index) -> tuple[str, str]:
    name = field_identifier(parameter.name)
    if parameter.type == "impl AsRef<str>":
        return f"{name}: impl AsRef<str>", f"{name}.as_ref()"
    if parameter.type == "&str":
        return f"{name}: &str", name
    owned, _ = _owned_parameter(parameter.type)
    return _argument(parameter.name, raw_index.qualified_type(owned))


def _request_name(resource: ResourceSpec, operation: OperationSpec) -> str:
    return (
        "".join(part.title() for part in operation.name.split("_"))
        + resource.name
        + "Request"
    )


def _parameter_request(
    resource: ResourceSpec, operation: OperationSpec, raw_index
) -> ParameterRequestSpec | None:
    if isinstance(operation.request_projection, JsonRequest):
        return None
    parameters = operation.raw_signature.parameters
    if not parameters or not any(
        _option(parameter.type) is not None for parameter in parameters
    ):
        return None
    fields = []
    for parameter in parameters:
        owned, _ = _owned_parameter(parameter.type)
        owned = raw_index.qualified_type(owned)
        optional = _option(owned)
        if optional is not None:
            setter_argument, setter_value = _argument(parameter.name, optional)
            fields.append(
                ParameterField(
                    parameter.name,
                    owned,
                    None,
                    None,
                    setter_argument,
                    setter_value,
                )
            )
        else:
            constructor_argument, constructor_value = _argument(parameter.name, owned)
            fields.append(
                ParameterField(
                    parameter.name,
                    owned,
                    constructor_argument,
                    constructor_value,
                    None,
                    None,
                )
            )
    return ParameterRequestSpec(_request_name(resource, operation), tuple(fields))


def _operation_call(
    resource: ResourceSpec, operation: OperationSpec, raw_index
) -> OperationCall:
    parameters = operation.raw_signature.parameters
    request = operation.request_projection
    if isinstance(request, JsonRequest):
        body = "request.into_raw()"
        if request.overrides:
            assignments = []
            for field, configured in request.overrides:
                if configured is True:
                    value_expression = "Some(true)"
                elif configured is False:
                    value_expression = "Some(false)"
                elif configured is None:
                    value_expression = "None"
                else:
                    raise OperationLoweringError(
                        f"unsupported request override literal for {field}"
                    )
                assignments.append(f"raw.{field} = {value_expression};")
            body = f"{{ let mut raw = request.into_raw(); {' '.join(assignments)} raw }}"
        declarations, values = [], []
        for parameter in parameters:
            if parameter.type == request.raw:
                declarations.append(f"request: {request.model}")
                values.append(body)
                continue
            declaration, value = _direct_parameter(parameter, raw_index)
            declarations.append(declaration)
            values.append(value)
        return OperationCall(", ".join(declarations), ", ".join(values), None)
    if not parameters:
        return OperationCall("", "", None)
    if not any(_option(parameter.type) is not None for parameter in parameters):
        declarations, values = [], []
        for parameter in parameters:
            declaration, value = _direct_parameter(parameter, raw_index)
            declarations.append(declaration)
            values.append(value)
        return OperationCall(", ".join(declarations), ", ".join(values), None)
    values = []
    for parameter in parameters:
        _, borrowed = _owned_parameter(parameter.type)
        value = f"request.{parameter.name}"
        if borrowed:
            value += (
                ".as_deref()"
                if _option(parameter.type) is not None
                else ".as_str()"
            )
        values.append(value)
    default = None
    if all(_option(parameter.type) is not None for parameter in parameters):
        default = ", ".join(
            "None::<&str>"
            if parameter.type == "Option<impl AsRef<str>>"
            else "None"
            for parameter in parameters
        )
    return OperationCall(
        f"request: {_request_name(resource, operation)}",
        ", ".join(values),
        default,
    )


def resolve_operations(ir: FacadeIr, raw_index) -> FacadeIr:
    """Return an equivalent IR whose operation rendering inputs are complete."""
    resources = []
    for resource in ir.resources:
        operations = tuple(
            replace(
                operation,
                call=_operation_call(resource, operation, raw_index),
                parameter_request=_parameter_request(resource, operation, raw_index),
            )
            for operation in resource.operations
        )
        resources.append(replace(resource, operations=operations))
    return replace(ir, resources=tuple(resources))
