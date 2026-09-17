"""Deterministic Rust emission from resolved facade IR.

This module is deliberately source-agnostic: it does not inspect OpenAPI,
generated raw Rust, syntax trees, or generator-specific module conventions.
"""

from .sdk_ir import (AccessorKind, AliasModelSpec, ArgumentKind, ArgumentSpec,
                     BinaryResponse, CollectIntoValue, EmptyResponse, EnumValue,
                     FacadeIr, IntoModelValue, IntoStringValue, JsonResponse,
                     LiteralValue, MapIntoValue, MapModelSpec, ModelSpec, OperationSpec,
                     ScalarEnumModelSpec, ResourceSpec, SimpleUnionModelSpec,
                     SomeValue, SseResponse, StructValue, UnionModelSpec,
                     VariableValue, ViewModelSpec, WrapperModelSpec)
from .sdk_raw_ir import RawBindingLayout
from .sdk_runtime import RustFacadeRuntime


def _indent(value: str, spaces: int = 4) -> str:
    prefix = " " * spaces
    return "\n".join(prefix + line if line else line for line in value.splitlines())


def _emit_argument(argument: ArgumentSpec) -> str:
    if argument.kind == ArgumentKind.EXACT:
        return f"{argument.name}: {argument.type}"
    if argument.kind == ArgumentKind.INTO_STRING:
        return f"{argument.name}: impl Into<String>"
    if argument.kind == ArgumentKind.INTO_MODEL:
        return f"{argument.name}: impl Into<{argument.type}>"
    if argument.kind == ArgumentKind.INTO_ITER_MODEL:
        return f"{argument.name}: impl IntoIterator<Item = {argument.type}>"
    raise TypeError(f"unsupported resolved argument kind: {argument.kind}")


def _map_into(value: str, depth: int) -> str:
    if depth == 0:
        return f"{value}.into()"
    return f"{value}.into_iter().map(|value| {_map_into('value', depth - 1)}).collect()"


def _emit_value(value) -> str:
    if isinstance(value, VariableValue):
        return value.name
    if isinstance(value, IntoStringValue):
        return f"{value.name}.into()"
    if isinstance(value, IntoModelValue):
        return f"Into::<{value.adapter}>::into({value.name}).into()"
    if isinstance(value, CollectIntoValue):
        return f"{value.name}.into_iter().map(Into::into).collect()"
    if isinstance(value, MapIntoValue):
        return _map_into(value.name, value.depth)
    if isinstance(value, SomeValue):
        rendered = _emit_value(value.value)
        for _ in range(value.depth):
            rendered = f"Some({rendered})"
        return rendered
    if isinstance(value, EnumValue):
        return f"{value.type}::{value.variant}({_emit_value(value.value)})"
    if isinstance(value, StructValue):
        assignments = []
        for field in value.fields:
            if field.shorthand:
                assignments.append(field.name)
            else:
                assignments.append(f"{field.name}: {_emit_value(field.value)}")
        return f"{value.type} {{ {', '.join(assignments)} }}"
    if isinstance(value, LiteralValue):
        return value.value
    raise TypeError(f"unsupported resolved value: {type(value).__name__}")


def _emit_wrapper(model: ModelSpec, spec: WrapperModelSpec) -> str:
    methods = []
    if spec.constructor is not None:
        arguments = ", ".join(_emit_argument(argument)
                              for argument in spec.constructor.arguments)
        methods.append(
            f"pub fn new({arguments}) -> Self {{\n"
            f"    Self {{ raw: {_emit_value(spec.constructor.value)} }}\n}}"
        )
    if spec.factories:
        methods.append("\n\n".join(
            f"pub fn {factory.name}({', '.join(_emit_argument(argument) for argument in factory.arguments)}) -> Self {{ "
            f"Self {{ raw: {_emit_value(factory.value)} }} }}"
            for factory in spec.factories
        ))
    if spec.setters:
        setters = []
        for setter in spec.setters:
            setters.append(
                f"#[must_use]\npub fn {setter.name}(mut self, {_emit_argument(setter.argument)}) -> Self {{\n"
                f"    self.raw.{setter.raw_field} = {_emit_value(setter.value)};\n    self\n}}"
            )
            if setter.null_name is not None:
                setters.append(
                    f"#[must_use]\npub fn {setter.null_name}(mut self) -> Self {{\n"
                    f"    self.raw.{setter.raw_field} = Some(None);\n    self\n}}"
                )
        methods.append("\n\n".join(setters))
    methods.extend((
        f"pub fn from_raw(raw: {model.raw}) -> Self {{ Self {{ raw }} }}",
        f"pub fn as_raw(&self) -> &{model.raw} {{ &self.raw }}",
        f"pub fn into_raw(self) -> {model.raw} {{ self.raw }}",
    ))
    default = (
        f"\n\nimpl Default for {model.name} {{\n"
        f"    fn default() -> Self {{ Self::new() }}\n}}"
        if spec.default else ""
    )
    return (
        f"#[derive(Debug, Clone)]\npub struct {model.name} {{ raw: {model.raw} }}\n\n"
        f"impl {model.name} {{\n{_indent(chr(10).join(methods))}\n}}\n\n"
        f"impl From<{model.raw}> for {model.name} {{\n"
        f"    fn from(raw: {model.raw}) -> Self {{ Self {{ raw }} }}\n}}\n\n"
        f"impl From<{model.name}> for {model.raw} {{\n"
        f"    fn from(value: {model.name}) -> Self {{ value.into_raw() }}\n}}" + default
    )


def _emit_union(model: ModelSpec, spec: UnionModelSpec) -> str:
    variants = [f"{branch.public_name}({branch.public_type})" for branch in spec.branches]
    constructors = [
        f"pub fn {branch.constructor_name}({_emit_argument(branch.argument)}) -> Self {{ "
        f"Self::{branch.public_name}({branch.argument.name}"
        + (".into()" if branch.argument.kind == ArgumentKind.INTO_STRING else "") + ") }"
        for branch in spec.branches
    ]
    branches = {branch.raw_payload: branch for branch in spec.branches}
    conversions = []
    for target in spec.targets:
        arms = []
        for raw_payload, raw_variant in target.variants:
            branch = branches[raw_payload]
            binding = branch.argument.name
            arms.append(
                f"{model.name}::{branch.public_name}({binding}) => "
                f"Self::{raw_variant}({_emit_value(branch.raw_value)})"
            )
        conversions.append(
            f"impl From<{model.name}> for {target.raw} {{\n"
            f"    fn from(value: {model.name}) -> Self {{\n        match value {{\n"
            f"{_indent(','.join(arms), 12)}\n        }}\n    }}\n}}"
        )
    return (
        f"#[derive(Debug, Clone)]\n#[non_exhaustive]\npub enum {model.name} {{\n"
        f"{_indent(','.join(variants))}\n}}\n\n"
        f"impl {model.name} {{\n{_indent(chr(10).join(constructors))}\n}}\n\n"
        + "\n\n".join(conversions)
    )


def _adapt_value(depth: int | None, value: str = "value") -> str:
    return value if depth is None else _map_into(value, depth)


def _emit_simple_union(model: ModelSpec, spec: SimpleUnionModelSpec) -> str:
    variants, arms, reverse_arms, conversions = [], [], [], []
    seen_payloads = set()
    for branch in spec.branches:
        variants.append(f"{branch.public_name}({branch.public_type})")
        raw_value = _adapt_value(branch.adapt_depth)
        arms.append(
            f"{model.name}::{branch.public_name}(value) => "
            f"Self::{branch.raw_name}({raw_value})"
        )
        reverse_arms.append(
            f"{model.raw}::{branch.raw_name}(value) => "
            f"Self::{branch.public_name}({raw_value})"
        )
        if branch.public_type not in seen_payloads:
            conversions.append(
                f"impl From<{branch.public_type}> for {model.name} {{\n"
                f"    fn from(value: {branch.public_type}) -> Self {{ "
                f"Self::{branch.public_name}(value) }}\n}}"
            )
            seen_payloads.add(branch.public_type)
        if branch.public_type == "String":
            conversions.append(
                f"impl From<&str> for {model.name} {{\n"
                f"    fn from(value: &str) -> Self {{ "
                f"Self::{branch.public_name}(value.into()) }}\n}}"
            )
    reverse = (
        f"\n\nimpl From<{model.raw}> for {model.name} {{\n"
        f"    fn from(value: {model.raw}) -> Self {{ match value {{\n"
        f"{_indent(','.join(reverse_arms), 8)}\n    }} }}\n}}"
        if spec.bidirectional else ""
    )
    return (
        f"#[derive(Debug, Clone)]\n#[non_exhaustive]\npub enum {model.name} {{\n"
        f"{_indent(','.join(variants))}\n}}\n\n"
        + "\n\n".join(conversions) + "\n\n"
        f"impl From<{model.name}> for {model.raw} {{\n"
        f"    fn from(value: {model.name}) -> Self {{ match value {{\n"
        f"{_indent(','.join(arms), 8)}\n    }} }}\n}}" + reverse
    )


def _direct_path(path: tuple[str, ...]) -> str:
    return "self.raw" + "".join(f".{field}" for field in path)


def _emit_accessor(accessor) -> str:
    expression = _direct_path(accessor.path)
    if accessor.kind == AccessorKind.COPY:
        return f"pub fn {accessor.name}(&self) -> {accessor.return_type} {{ {expression} }}"
    if accessor.kind == AccessorKind.REF:
        return f"pub fn {accessor.name}(&self) -> {accessor.return_type} {{ &{expression} }}"
    if accessor.kind == AccessorKind.OPTIONAL_COPY:
        return f"pub fn {accessor.name}(&self) -> {accessor.return_type} {{ {expression} }}"
    if accessor.kind == AccessorKind.OPTIONAL_REF:
        return f"pub fn {accessor.name}(&self) -> {accessor.return_type} {{ {expression}.as_deref() }}"
    if accessor.kind == AccessorKind.ITER:
        return (
            f"pub fn {accessor.name}(&self) -> {accessor.return_type} {{\n"
            f"    {expression}.iter().map({accessor.wrapper}::new)\n}}"
        )
    if accessor.kind == AccessorKind.FIRST_STRING_VARIANT:
        expression = "self.raw"
        for segment in accessor.path:
            expression += (".first()?" if segment == "first" else
                           ".as_ref()?" if segment == "optional" else f".{segment}")
        return (
            f"pub fn {accessor.name}(&self) -> {accessor.return_type} {{\n"
            f"    match {expression} {{ {accessor.enum_type}::{accessor.enum_variant}(value) => Some(value), _ => None }}\n}}"
        )
    raise TypeError(f"unsupported resolved accessor kind: {accessor.kind}")


def _emit_view(model: ModelSpec, spec: ViewModelSpec) -> str:
    accessors = "\n".join(_emit_accessor(accessor) for accessor in spec.accessors)
    if spec.borrowed:
        return (
            f"#[derive(Debug, Clone, Copy)]\npub struct {model.name}<'a> {{ raw: &'a {model.raw} }}\n\n"
            f"impl<'a> {model.name}<'a> {{\n    pub(crate) fn new(raw: &'a {model.raw}) -> Self {{ Self {{ raw }} }}\n"
            f"{_indent(accessors)}\n    pub fn raw(&self) -> &'a {model.raw} {{ self.raw }}\n}}"
        )
    return (
        f"#[derive(Debug, Clone)]\npub struct {model.name} {{ raw: {model.raw} }}\n\n"
        f"impl {model.name} {{\n{_indent(accessors)}\n"
        f"    pub fn raw(&self) -> &{model.raw} {{ &self.raw }}\n"
        f"    pub fn into_raw(self) -> {model.raw} {{ self.raw }}\n}}\n\n"
        f"impl From<{model.raw}> for {model.name} {{ fn from(raw: {model.raw}) -> Self {{ Self {{ raw }} }} }}\n\n"
        f"impl From<{model.name}> for {model.raw} {{ fn from(value: {model.name}) -> Self {{ value.into_raw() }} }}"
    )


def _emit_map(model: ModelSpec, spec: MapModelSpec) -> str:
    return (
        f"#[derive(Debug, Clone, Default)]\npub struct {model.name} {{ values: {spec.public_type} }}\n\n"
        f"impl {model.name} {{\n"
        f"    pub fn new(values: {spec.public_type}) -> Self {{ Self {{ values }} }}\n"
        f"    pub fn as_map(&self) -> &{spec.public_type} {{ &self.values }}\n"
        f"    pub fn into_map(self) -> {spec.public_type} {{ self.values }}\n"
        f"}}\n\n"
        f"impl From<{spec.public_type}> for {model.name} {{\n"
        f"    fn from(values: {spec.public_type}) -> Self {{ Self {{ values }} }}\n"
        f"}}\n\n"
        f"impl From<{model.raw}> for {model.name} {{\n"
        f"    fn from(value: {model.raw}) -> Self {{ Self {{ values: value.{spec.raw_field} }} }}\n"
        f"}}\n\n"
        f"impl From<{model.name}> for {model.raw} {{\n"
        f"    fn from(value: {model.name}) -> Self {{ Self {{ {spec.raw_field}: value.values }} }}\n"
        f"}}"
    )


def _emit_scalar_enum(model: ModelSpec, spec: ScalarEnumModelSpec) -> str:
    variants = ",".join(spec.variants)
    arms = ",".join(
        f"{model.name}::{variant} => Self::{variant}" for variant in spec.variants
    )
    return (
        f"#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n"
        f"#[non_exhaustive]\npub enum {model.name} {{\n"
        f"{_indent(variants)}\n}}\n\n"
        f"impl From<{model.name}> for {model.raw} {{\n"
        f"    fn from(value: {model.name}) -> Self {{ match value {{\n"
        f"{_indent(arms, 8)}\n    }} }}\n}}"
    )


def emit_model(model: ModelSpec) -> str:
    spec = model.render
    if isinstance(spec, WrapperModelSpec):
        return _emit_wrapper(model, spec)
    if isinstance(spec, UnionModelSpec):
        return _emit_union(model, spec)
    if isinstance(spec, SimpleUnionModelSpec):
        return _emit_simple_union(model, spec)
    if isinstance(spec, ViewModelSpec):
        return _emit_view(model, spec)
    if isinstance(spec, AliasModelSpec):
        return f"pub type {model.name} = {spec.public_type};"
    if isinstance(spec, MapModelSpec):
        return _emit_map(model, spec)
    if isinstance(spec, ScalarEnumModelSpec):
        return _emit_scalar_enum(model, spec)
    raise TypeError(f"model {model.name} has not been resolved for rendering")


def _emit_parameter_request(operation: OperationSpec) -> str:
    request = operation.parameter_request
    if request is None:
        return ""
    fields = [f"{field.name}: {field.type}" for field in request.fields]
    arguments = [field.constructor_argument for field in request.fields
                 if field.constructor_argument is not None]
    values = [
        f"{field.name}: {field.constructor_value}"
        if field.constructor_value is not None else f"{field.name}: None"
        for field in request.fields
    ]
    setters = []
    for field in request.fields:
        if field.setter_argument is None or field.setter_value is None:
            continue
        setters.append(
            f"#[must_use] pub fn {field.name}(mut self, {field.setter_argument}) -> Self {{ "
            f"self.{field.name} = Some({field.setter_value}); self }}"
        )
    derives = "Debug, Clone" + (", Default" if not arguments else "")
    return (
        f"#[derive({derives})]\npub struct {request.name} {{ {', '.join(fields)} }}\n"
        f"impl {request.name} {{ pub fn new({', '.join(arguments)}) -> Self {{ Self {{ {', '.join(values)} }} }}\n"
        + "\n".join(setters) + "\n}\n"
    )


def emit_operation(operation: OperationSpec, runtime: RustFacadeRuntime) -> str:
    if operation.call is None:
        raise TypeError(f"operation {operation.operation_id} has not been resolved for rendering")
    arguments = operation.call.arguments
    call = operation.call.raw_arguments
    response = operation.response_projection
    error = runtime.error_type
    if isinstance(response, SseResponse):
        stream = response.stream
        separator = ", " if arguments else ""
        return (
            f"pub async fn {operation.name}(&self{separator}{arguments}) -> Result<{stream.type}, {error}> {{\n"
            f"    let bytes = self.raw.{operation.raw_method}({call}).await.map_err({error}::from)?;\n"
            f"    let events = {runtime.sse_module}::{runtime.sse_function}::<_, _, {stream.item}>(bytes)\n"
            f"        .map(|event| event.map(|event| {stream.wrapper}::from(event.data)).map_err(Into::into));\n"
            f"    Ok(Box::pin(events))\n}}"
        )
    separator = ", " if arguments else ""
    if isinstance(response, EmptyResponse):
        return (
            f"pub async fn {operation.name}(&self{separator}{arguments}) -> Result<(), {error}> {{\n"
            f"    self.raw.{operation.raw_method}({call}).await.map_err(Into::into)\n}}"
        )
    if isinstance(response, BinaryResponse):
        return (
            f"pub async fn {operation.name}(&self{separator}{arguments}) -> Result<BinaryStream, {error}> {{\n"
            f"    let bytes = self.raw.{operation.raw_method}({call}).await.map_err({error}::from)?;\n"
            f"    let chunks = bytes.map(|chunk| chunk.map_err(Into::into));\n"
            f"    Ok(Box::pin(chunks))\n}}"
        )
    if not isinstance(response, JsonResponse):
        raise TypeError(f"unsupported resolved response projection: {type(response).__name__}")
    if operation.call.default_raw_arguments is not None:
        configured = (
            f"pub async fn {operation.name}_with(&self, {arguments}) -> Result<{response.model}, {error}> {{\n"
            f"    self.raw.{operation.raw_method}({call}).await.map(Into::into).map_err(Into::into)\n}}"
        )
        return (
            f"pub async fn {operation.name}(&self) -> Result<{response.model}, {error}> {{\n"
            f"    self.raw.{operation.raw_method}({operation.call.default_raw_arguments}).await.map(Into::into).map_err(Into::into)\n}}\n\n"
            + configured
        )
    return (
        f"pub async fn {operation.name}(&self{separator}{arguments}) -> Result<{response.model}, {error}> {{\n"
        f"    self.raw.{operation.raw_method}({call}).await.map(Into::into).map_err(Into::into)\n}}"
    )


def _prelude_imports(binding: RawBindingLayout) -> str:
    return "".join(f"use {path};\n" for path in binding.type_preludes)


def _client_name(binding: RawBindingLayout) -> str:
    return binding.client.type_path.rsplit("::", 1)[-1]


def emit_resource(
    resource: ResourceSpec,
    resources: tuple[ResourceSpec, ...],
    binding: RawBindingLayout,
    runtime: RustFacadeRuntime,
) -> str:
    streaming = any(isinstance(operation.response_projection, SseResponse)
                    for operation in resource.operations)
    binary = any(isinstance(operation.response_projection, BinaryResponse)
                 for operation in resource.operations)
    imports = "use futures_util::StreamExt;\n" if streaming or binary else ""
    if streaming:
        imports += _prelude_imports(binding)
    operations = "\n\n".join(
        emit_operation(operation, runtime) for operation in resource.operations
    )
    requests = "\n".join(_emit_parameter_request(operation) for operation in resource.operations)
    children = sorted(
        (candidate for candidate in resources
         if len(candidate.path) == len(resource.path) + 1 and candidate.path[:-1] == resource.path),
        key=lambda candidate: candidate.path,
    )
    child_accessors = "\n\n".join(
        f"pub fn {child.path[-1]}(&self) -> {child.name}<'a> {{ {child.name}::new(self.raw) }}"
        for child in children
    )
    members = "\n\n".join(part for part in (child_accessors, operations) if part)
    client = _client_name(binding)
    return runtime.generated_marker + imports + (
        f"use super::*;\nuse {binding.client.type_path};\n"
    ) + requests + (
        f"#[derive(Clone, Copy)]\npub struct {resource.name}<'a> {{ raw: &'a {client} }}\n\n"
        f"impl<'a> {resource.name}<'a> {{\n"
        f"    pub(crate) fn new(raw: &'a {client}) -> Self {{ Self {{ raw }} }}\n"
        f"{_indent(members)}\n}}\n"
    )


def emit_mod(
    ir: FacadeIr,
    binding: RawBindingLayout,
    runtime: RustFacadeRuntime,
) -> str:
    declarations = "\n".join(
        ("pub mod " if len(resource.path) == 1 else "mod ") + f"{resource.module};"
        for resource in ir.resources
    )
    resources = "\n".join(f"pub use {resource.module}::{resource.name};" for resource in ir.resources)
    exported_types = [model.name for model in ir.models]
    exported_types.extend(
        operation.response_projection.stream.type
        for resource in ir.resources for operation in resource.operations
        if isinstance(operation.response_projection, SseResponse)
    )
    if any(isinstance(operation.response_projection, BinaryResponse)
           for resource in ir.resources for operation in resource.operations):
        exported_types.append("BinaryStream")
    models = ", ".join(exported_types)
    accessors = "\n".join(
        f"pub fn {resource.path[0]}(&self) -> {resource.name}<'_> {{ {resource.name}::new(&self.raw) }}"
        for resource in ir.resources if len(resource.path) == 1
    )
    exports = ", ".join(runtime.error_exports)
    client = _client_name(binding)
    raw = binding.client
    return runtime.generated_marker + f"""{declarations}
pub mod {runtime.error_module};
mod facade_types;

{resources}
pub use {runtime.error_module}::{{{exports}}};
pub use facade_types::{{{models}}};

use {raw.type_path};

#[derive(Clone)]
pub struct {ir.client_name} {{ raw: {client} }}

impl {ir.client_name} {{
    pub fn new(api_key: impl Into<String>) -> Self {{
        Self {{ raw: {client}::{raw.constructor}().{raw.api_key_builder}(api_key) }}
    }}
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {{
        self.raw = self.raw.{raw.base_url_builder}(base_url);
        self
    }}
{_indent(accessors)}
    pub fn raw(&self) -> &{client} {{ &self.raw }}
}}
"""


def emit_facade_types(
    ir: FacadeIr,
    binding: RawBindingLayout,
    runtime: RustFacadeRuntime,
) -> str:
    source = (
        runtime.generated_marker
        + "use std::pin::Pin;\nuse futures_util::Stream;\n"
        + f"use super::{runtime.error_type};\n"
        + _prelude_imports(binding)
        + "\n"
    )
    source += "\n\n".join(emit_model(model) for model in ir.models)
    aliases = []
    if any(isinstance(operation.response_projection, BinaryResponse)
           for resource in ir.resources for operation in resource.operations):
        aliases.append(
            f"pub type BinaryStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, {runtime.error_type}>> + Send + 'static>>;"
        )
    for resource in ir.resources:
        for operation in resource.operations:
            if isinstance(operation.response_projection, SseResponse):
                stream = operation.response_projection.stream
                aliases.append(
                    f"pub type {stream.type} = Pin<Box<dyn Stream<Item = Result<{stream.wrapper}, {runtime.error_type}>> + Send + 'static>>;"
                )
    if aliases:
        source += "\n\n" + "\n".join(aliases) + "\n"
    return source


def emit(
    ir: FacadeIr,
    binding: RawBindingLayout,
    runtime: RustFacadeRuntime,
) -> dict[str, str]:
    """Render a fully resolved facade IR into its deterministic Rust file set."""
    files = {
        "facade_types.rs": emit_facade_types(ir, binding, runtime),
        "mod.rs": emit_mod(ir, binding, runtime),
    }
    for resource in ir.resources:
        filename = f"{resource.module}.rs"
        if filename in files or filename == f"{runtime.error_module}.rs":
            raise ValueError(f"resource collides with reserved output {filename}")
        files[filename] = emit_resource(resource, ir.resources, binding, runtime)
    return files
