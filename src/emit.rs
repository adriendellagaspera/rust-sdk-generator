use std::collections::{BTreeMap, BTreeSet};

use crate::contracts::{BindingLayout, Bindings, Runtime};
use crate::error::{GenerationError, Result};
use crate::ir::*;

fn error(code: &'static str, message: impl Into<String>) -> GenerationError {
    GenerationError::new(code, message)
}

fn qualify_transport_type(spelling: &mut String, bindings: &Bindings) -> Result<()> {
    *spelling = bindings.qualified_type(spelling)?;
    Ok(())
}

fn qualify_struct_value(value: &mut StructValue, bindings: &Bindings) -> Result<()> {
    qualify_transport_type(&mut value.type_name, bindings)?;
    for field in &mut value.fields {
        qualify_value(&mut field.value, bindings)?;
    }
    Ok(())
}

fn qualify_value(value: &mut ValueSpec, bindings: &Bindings) -> Result<()> {
    match value {
        ValueSpec::Some { value, .. } => qualify_value(value, bindings)?,
        ValueSpec::Enum {
            type_name, value, ..
        } => {
            qualify_transport_type(type_name, bindings)?;
            qualify_value(value, bindings)?;
        }
        ValueSpec::Struct(value) => qualify_struct_value(value, bindings)?,
        ValueSpec::Variable(_)
        | ValueSpec::IntoString(_)
        | ValueSpec::IntoModel { .. }
        | ValueSpec::CollectInto(_)
        | ValueSpec::MapInto { .. }
        | ValueSpec::OptionMapInto { .. }
        | ValueSpec::AdaptInto { .. }
        | ValueSpec::OptionAdaptInto { .. }
        | ValueSpec::Literal(_) => {}
    }
    Ok(())
}

fn qualify_transport_symbols(ir: &mut FacadeIr, bindings: &Bindings) -> Result<()> {
    for model in &mut ir.models {
        qualify_transport_type(&mut model.raw, bindings)?;
        match &mut model.render {
            ModelRenderSpec::Wrapper(spec) => {
                if let Some(constructor) = &mut spec.constructor {
                    qualify_struct_value(&mut constructor.value, bindings)?;
                }
                for factory in &mut spec.factories {
                    qualify_struct_value(&mut factory.value, bindings)?;
                }
                for setter in &mut spec.setters {
                    qualify_value(&mut setter.value, bindings)?;
                }
            }
            ModelRenderSpec::Union(spec) => {
                for branch in &mut spec.branches {
                    qualify_value(&mut branch.raw_value, bindings)?;
                }
                for target in &mut spec.targets {
                    qualify_transport_type(&mut target.raw, bindings)?;
                }
            }
            ModelRenderSpec::View(spec) => {
                for accessor in &mut spec.accessors {
                    if let Some(enum_type) = &mut accessor.enum_type {
                        qualify_transport_type(enum_type, bindings)?;
                    }
                }
            }
            ModelRenderSpec::SimpleUnion(_)
            | ModelRenderSpec::Alias(_)
            | ModelRenderSpec::Map(_)
            | ModelRenderSpec::Collection(_)
            | ModelRenderSpec::ScalarEnum(_) => {}
        }
    }
    for resource in &mut ir.resources {
        for operation in &mut resource.operations {
            if let ResponseProjection::Sse(stream) = &mut operation.response_projection {
                if stream.variants.is_empty() {
                    qualify_transport_type(&mut stream.item, bindings)?;
                } else {
                    for variant in &mut stream.variants {
                        qualify_transport_type(&mut variant.raw, bindings)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn indent(value: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    value
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn emit_argument(argument: &ArgumentSpec) -> String {
    match argument.kind {
        ArgumentKind::Exact => format!("{}: {}", argument.name, argument.type_name),
        ArgumentKind::IntoString => format!("{}: impl Into<String>", argument.name),
        ArgumentKind::IntoModel => format!("{}: impl Into<{}>", argument.name, argument.type_name),
        ArgumentKind::IntoIterModel => format!(
            "{}: impl IntoIterator<Item = {}>",
            argument.name, argument.type_name
        ),
    }
}

fn map_into_raw(value: &str, depth: usize) -> String {
    if depth == 0 {
        format!("__RustSdkIntoRaw::into_raw({value})")
    } else {
        format!(
            "{value}.into_iter().map(|value| {}).collect()",
            map_into_raw("value", depth - 1)
        )
    }
}

fn adapt_into_raw_plan(value: &str, plan: &AdaptPlan) -> String {
    match plan {
        AdaptPlan::Direct => format!("__RustSdkIntoRaw::into_raw({value})"),
        AdaptPlan::Vec(inner) => format!(
            "{value}.into_iter().map(|value| {}).collect()",
            adapt_into_raw_plan("value", inner)
        ),
        AdaptPlan::Boxed(inner) => {
            format!("Box::new({})", adapt_into_raw_plan("*value", inner))
        }
    }
}

fn adapt_from_raw_plan(value: &str, plan: &AdaptPlan) -> String {
    match plan {
        AdaptPlan::Direct => format!("__RustSdkFromRaw::from_raw({value})"),
        AdaptPlan::Vec(inner) => format!(
            "{value}.into_iter().map(|value| {}).collect()",
            adapt_from_raw_plan("value", inner)
        ),
        AdaptPlan::Boxed(inner) => {
            format!("Box::new({})", adapt_from_raw_plan("*value", inner))
        }
    }
}

fn emit_value(value: &ValueSpec) -> String {
    match value {
        ValueSpec::Variable(name) => name.clone(),
        ValueSpec::IntoString(name) => format!("{name}.into()"),
        ValueSpec::IntoModel { name, adapter } => {
            format!("__RustSdkIntoRaw::into_raw(Into::<{adapter}>::into({name}))")
        }
        ValueSpec::CollectInto(name) => {
            format!("{name}.into_iter().map(__RustSdkIntoRaw::into_raw).collect()")
        }
        ValueSpec::MapInto { name, depth } => map_into_raw(name, *depth),
        ValueSpec::OptionMapInto { name, depth } => {
            format!("{name}.map(|value| {})", map_into_raw("value", *depth))
        }
        ValueSpec::AdaptInto { name, plan } => adapt_into_raw_plan(name, plan),
        ValueSpec::OptionAdaptInto { name, plan } => {
            format!("{name}.map(|value| {})", adapt_into_raw_plan("value", plan))
        }
        ValueSpec::Some { value, depth } => {
            let mut rendered = emit_value(value);
            for _ in 0..*depth {
                rendered = format!("Some({rendered})");
            }
            rendered
        }
        ValueSpec::Enum {
            type_name,
            variant,
            value,
        } => format!("{type_name}::{variant}({})", emit_value(value)),
        ValueSpec::Struct(value) => emit_struct_value(value),
        ValueSpec::Literal(value) => value.clone(),
    }
}

fn emit_struct_value(value: &StructValue) -> String {
    let assignments = value
        .fields
        .iter()
        .map(|field| {
            if field.shorthand {
                field.name.clone()
            } else {
                format!("{}: {}", field.name, emit_value(&field.value))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{} {{ {assignments} }}", value.type_name)
}

fn emit_wrapper(model: &ModelSpec, spec: &WrapperModelSpec) -> String {
    let mut methods = Vec::new();
    if let Some(constructor) = &spec.constructor {
        let arguments = constructor
            .arguments
            .iter()
            .map(emit_argument)
            .collect::<Vec<_>>()
            .join(", ");
        methods.push(format!(
            "pub fn new({arguments}) -> Self {{\n    Self {{ raw: {} }}\n}}",
            emit_struct_value(&constructor.value)
        ));
    }
    if !spec.factories.is_empty() {
        methods.push(
            spec.factories
                .iter()
                .map(|factory| {
                    format!(
                        "pub fn {}({}) -> Self {{ Self {{ raw: {} }} }}",
                        factory.name,
                        factory
                            .arguments
                            .iter()
                            .map(emit_argument)
                            .collect::<Vec<_>>()
                            .join(", "),
                        emit_struct_value(&factory.value)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
    }
    if !spec.setters.is_empty() {
        methods.push(
            spec.setters
                .iter()
                .flat_map(|setter| {
                    let mut setters = vec![format!(
                        "#[must_use]\npub fn {}(mut self, {}) -> Self {{\n    self.raw.{} = {};\n    self\n}}",
                        setter.name,
                        emit_argument(&setter.argument),
                        setter.raw_field,
                        emit_value(&setter.value)
                    )];
                    if let Some(null_name) = &setter.null_name {
                        setters.push(format!(
                            "#[must_use]\npub fn {null_name}(mut self) -> Self {{\n    self.raw.{} = Some(None);\n    self\n}}",
                            setter.raw_field
                        ));
                    }
                    setters
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
    }

    let default = if spec.default {
        format!(
            "\n\nimpl Default for {} {{\n    fn default() -> Self {{ Self::new() }}\n}}",
            model.name
        )
    } else {
        String::new()
    };
    format!(
        "#[derive(Debug, Clone)]\npub struct {} {{ raw: {} }}\n\nimpl {} {{\n{}\n}}\n\nimpl __RustSdkFromRaw<{}> for {} {{\n    fn from_raw(raw: {}) -> Self {{ Self {{ raw }} }}\n}}\n\nimpl __RustSdkIntoRaw<{}> for {} {{\n    fn into_raw(self) -> {} {{ self.raw }}\n}}{}",
        model.name,
        model.raw,
        model.name,
        indent(&methods.join("\n"), 4),
        model.raw,
        model.name,
        model.raw,
        model.raw,
        model.name,
        model.raw,
        default
    )
}

fn emit_union(model: &ModelSpec, spec: &UnionModelSpec) -> String {
    let variants = spec
        .branches
        .iter()
        .map(|branch| format!("{}({})", branch.public_name, branch.public_type))
        .collect::<Vec<_>>()
        .join(",");
    let constructors = spec
        .branches
        .iter()
        .map(|branch| {
            let suffix = if branch.argument.kind == ArgumentKind::IntoString {
                ".into()"
            } else {
                ""
            };
            format!(
                "pub fn {}({}) -> Self {{ Self::{}({}{suffix}) }}",
                branch.constructor_name,
                emit_argument(&branch.argument),
                branch.public_name,
                branch.argument.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let branches: BTreeMap<_, _> = spec
        .branches
        .iter()
        .map(|branch| (branch.raw_payload.as_str(), branch))
        .collect();
    let conversions = spec
        .targets
        .iter()
        .map(|target| {
            let arms = target
                .variants
                .iter()
                .map(|(raw_payload, raw_variant)| {
                    let branch = branches[raw_payload.as_str()];
                    format!(
                        "{}::{}({}) => {}::{}({})",
                        model.name,
                        branch.public_name,
                        branch.argument.name,
                        target.raw,
                        raw_variant,
                        emit_value(&branch.raw_value)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "impl __RustSdkIntoRaw<{}> for {} {{\n    fn into_raw(self) -> {} {{\n        match self {{\n{}\n        }}\n    }}\n}}",
                target.raw,
                model.name,
                target.raw,
                indent(&arms, 12)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "#[derive(Debug, Clone)]\n#[non_exhaustive]\npub enum {} {{\n{}\n}}\n\nimpl {} {{\n{}\n}}\n\n{}",
        model.name,
        indent(&variants, 4),
        model.name,
        indent(&constructors, 4),
        conversions
    )
}

fn adapt_into_raw(plan: Option<&AdaptPlan>) -> String {
    plan.map_or_else(|| "value".into(), |plan| adapt_into_raw_plan("value", plan))
}

fn adapt_from_raw(plan: Option<&AdaptPlan>) -> String {
    plan.map_or_else(|| "value".into(), |plan| adapt_from_raw_plan("value", plan))
}

fn emit_simple_union(model: &ModelSpec, spec: &SimpleUnionModelSpec) -> String {
    let mut variants = Vec::new();
    let mut arms = Vec::new();
    let mut reverse_arms = Vec::new();
    let mut conversions = Vec::new();
    let mut seen_payloads = BTreeSet::new();
    for branch in &spec.branches {
        variants.push(format!("{}({})", branch.public_name, branch.public_type));
        let raw_value = adapt_into_raw(branch.adapt.as_ref());
        arms.push(format!(
            "{}::{}(value) => {}::{}({raw_value})",
            model.name, branch.public_name, model.raw, branch.raw_name
        ));
        let public_value = adapt_from_raw(branch.adapt.as_ref());
        reverse_arms.push(format!(
            "{}::{}(value) => Self::{}({public_value})",
            model.raw, branch.raw_name, branch.public_name
        ));
        if seen_payloads.insert(branch.public_type.clone()) {
            conversions.push(format!(
                "impl From<{}> for {} {{\n    fn from(value: {}) -> Self {{ Self::{}(value) }}\n}}",
                branch.public_type, model.name, branch.public_type, branch.public_name
            ));
        }
        if branch.public_type == "String" {
            conversions.push(format!(
                "impl From<&str> for {} {{\n    fn from(value: &str) -> Self {{ Self::{}(value.into()) }}\n}}",
                model.name, branch.public_name
            ));
        }
    }
    let reverse = if spec.bidirectional {
        format!(
            "\n\nimpl __RustSdkFromRaw<{}> for {} {{\n    fn from_raw(value: {}) -> Self {{ match value {{\n{}\n    }} }}\n}}",
            model.raw,
            model.name,
            model.raw,
            indent(&reverse_arms.join(","), 8)
        )
    } else {
        String::new()
    };
    format!(
        "#[derive(Debug, Clone)]\n#[non_exhaustive]\npub enum {} {{\n{}\n}}\n\n{}\n\nimpl __RustSdkIntoRaw<{}> for {} {{\n    fn into_raw(self) -> {} {{ match self {{\n{}\n    }} }}\n}}{}",
        model.name,
        indent(&variants.join(","), 4),
        conversions.join("\n\n"),
        model.raw,
        model.name,
        model.raw,
        indent(&arms.join(","), 8),
        reverse
    )
}

fn direct_path(path: &[String]) -> String {
    format!(
        "self.raw{}",
        path.iter()
            .map(|field| format!(".{field}"))
            .collect::<String>()
    )
}

fn emit_accessor(accessor: &ResolvedAccessor) -> String {
    let expression = direct_path(&accessor.path);
    match accessor.kind {
        crate::AccessorKindDefinition::Copy => format!(
            "pub fn {}(&self) -> {} {{ {expression} }}",
            accessor.name, accessor.return_type
        ),
        crate::AccessorKindDefinition::Ref => format!(
            "pub fn {}(&self) -> {} {{ &{expression} }}",
            accessor.name, accessor.return_type
        ),
        crate::AccessorKindDefinition::OptionalCopy => format!(
            "pub fn {}(&self) -> {} {{ {expression} }}",
            accessor.name, accessor.return_type
        ),
        crate::AccessorKindDefinition::OptionalRef => {
            let borrow = if accessor.return_type == "Option<&str>"
                || accessor.return_type.starts_with("Option<&[")
            {
                "as_deref"
            } else {
                "as_ref"
            };
            format!(
                "pub fn {}(&self) -> {} {{ {expression}.{borrow}() }}",
                accessor.name, accessor.return_type
            )
        }
        crate::AccessorKindDefinition::Iter => format!(
            "pub fn {}(&self) -> {} {{\n    {expression}.iter().map({}::new)\n}}",
            accessor.name,
            accessor.return_type,
            accessor.wrapper.as_deref().expect("resolved iter wrapper")
        ),
        crate::AccessorKindDefinition::View => format!(
            "pub fn {}(&self) -> {} {{ {}::new(&{expression}) }}",
            accessor.name,
            accessor.return_type,
            accessor.wrapper.as_deref().expect("view wrapper")
        ),
        crate::AccessorKindDefinition::OptionalView => format!(
            "pub fn {}(&self) -> {} {{ {expression}.as_ref().map({}::new) }}",
            accessor.name,
            accessor.return_type,
            accessor.wrapper.as_deref().expect("view wrapper")
        ),
        crate::AccessorKindDefinition::OptionalIter => format!(
            "pub fn {}(&self) -> {} {{ {expression}.as_ref().map(|items| items.iter().map({}::new)) }}",
            accessor.name,
            accessor.return_type,
            accessor.wrapper.as_deref().expect("iter wrapper")
        ),
        crate::AccessorKindDefinition::Converted => format!(
            "pub fn {}(&self) -> {} {{ __RustSdkFromRaw::from_raw({expression}.clone()) }}",
            accessor.name, accessor.return_type
        ),
        crate::AccessorKindDefinition::OptionalConverted => format!(
            "pub fn {}(&self) -> {} {{ {expression}.as_ref().map(|value| __RustSdkFromRaw::from_raw(value.clone())) }}",
            accessor.name, accessor.return_type
        ),
        crate::AccessorKindDefinition::ConvertedIter => format!(
            "pub fn {}(&self) -> {} {{ {expression}.iter().cloned().map(__RustSdkFromRaw::from_raw) }}",
            accessor.name, accessor.return_type
        ),
        crate::AccessorKindDefinition::FirstStringVariant => {
            let mut expression = "self.raw".to_owned();
            for segment in &accessor.path {
                if segment == "first" {
                    expression.push_str(".first()?");
                } else if segment == "optional" {
                    expression.push_str(".as_ref()?");
                } else {
                    expression.push('.');
                    expression.push_str(segment);
                }
            }
            format!(
                "pub fn {}(&self) -> {} {{\n    match {expression} {{ {}::{}(value) => Some(value), _ => None }}\n}}",
                accessor.name,
                accessor.return_type,
                accessor.enum_type.as_deref().expect("resolved enum type"),
                accessor
                    .enum_variant
                    .as_deref()
                    .expect("resolved enum variant")
            )
        }
    }
}

fn emit_view(model: &ModelSpec, spec: &ViewModelSpec) -> String {
    let accessors = spec
        .accessors
        .iter()
        .map(emit_accessor)
        .collect::<Vec<_>>()
        .join("\n");
    if spec.borrowed {
        format!(
            "#[derive(Debug, Clone, Copy)]\npub struct {}<'a> {{ raw: &'a {} }}\n\nimpl<'a> {}<'a> {{\n    pub(crate) fn new(raw: &'a {}) -> Self {{ Self {{ raw }} }}\n{}\n}}",
            model.name,
            model.raw,
            model.name,
            model.raw,
            indent(&accessors, 4)
        )
    } else {
        format!(
            "#[derive(Debug, Clone)]\npub struct {} {{ raw: {} }}\n\nimpl {} {{\n{}\n}}\n\nimpl __RustSdkFromRaw<{}> for {} {{ fn from_raw(raw: {}) -> Self {{ Self {{ raw }} }} }}\n\nimpl __RustSdkIntoRaw<{}> for {} {{ fn into_raw(self) -> {} {{ self.raw }} }}",
            model.name,
            model.raw,
            model.name,
            indent(&accessors, 4),
            model.raw,
            model.name,
            model.raw,
            model.raw,
            model.name,
            model.raw
        )
    }
}

fn emit_map(model: &ModelSpec, spec: &MapModelSpec) -> String {
    let from_raw_values = spec.value_adapt.as_ref().map_or_else(
        || format!("value.{}", spec.raw_field),
        |plan| {
            format!(
                "value.{}.into_iter().map(|(key, value)| (key, {})).collect()",
                spec.raw_field,
                adapt_from_raw_plan("value", plan)
            )
        },
    );
    let into_raw_values = spec.value_adapt.as_ref().map_or_else(
        || "self.values".into(),
        |plan| {
            format!(
                "self.values.into_iter().map(|(key, value)| (key, {})).collect()",
                adapt_into_raw_plan("value", plan)
            )
        },
    );
    format!(
        "#[derive(Debug, Clone, Default)]\npub struct {} {{ values: {} }}\n\nimpl {} {{\n    pub fn new(values: {}) -> Self {{ Self {{ values }} }}\n    pub fn as_map(&self) -> &{} {{ &self.values }}\n    pub fn into_map(self) -> {} {{ self.values }}\n}}\n\nimpl From<{}> for {} {{\n    fn from(values: {}) -> Self {{ Self {{ values }} }}\n}}\n\nimpl __RustSdkFromRaw<{}> for {} {{\n    fn from_raw(value: {}) -> Self {{ Self {{ values: {from_raw_values} }} }}\n}}\n\nimpl __RustSdkIntoRaw<{}> for {} {{\n    fn into_raw(self) -> {} {{ {} {{ {}: {into_raw_values} }} }}\n}}",
        model.name,
        spec.public_type,
        model.name,
        spec.public_type,
        spec.public_type,
        spec.public_type,
        spec.public_type,
        model.name,
        spec.public_type,
        model.raw,
        model.name,
        model.raw,
        model.raw,
        model.name,
        model.raw,
        model.raw,
        spec.raw_field
    )
}

fn emit_collection(model: &ModelSpec, spec: &CollectionModelSpec) -> String {
    let from_raw_values = spec
        .item_adapt
        .as_ref()
        .map_or_else(|| "value".into(), |plan| adapt_from_raw_plan("value", plan));
    let into_raw_values = spec.item_adapt.as_ref().map_or_else(
        || "self.values".into(),
        |plan| adapt_into_raw_plan("self.values", plan),
    );
    format!(
        "#[derive(Debug, Clone, Default)]\npub struct {} {{ values: {} }}\n\nimpl {} {{\n    pub fn new(values: {}) -> Self {{ Self {{ values }} }}\n    pub fn as_slice(&self) -> &[_] {{ &self.values }}\n    pub fn into_vec(self) -> {} {{ self.values }}\n}}\n\nimpl From<{}> for {} {{\n    fn from(values: {}) -> Self {{ Self {{ values }} }}\n}}\n\nimpl __RustSdkFromRaw<{}> for {} {{\n    fn from_raw(value: {}) -> Self {{ Self {{ values: {from_raw_values} }} }}\n}}\n\nimpl __RustSdkIntoRaw<{}> for {} {{\n    fn into_raw(self) -> {} {{ {into_raw_values} }}\n}}",
        model.name,
        spec.public_type,
        model.name,
        spec.public_type,
        spec.public_type,
        spec.public_type,
        model.name,
        spec.public_type,
        model.raw,
        model.name,
        model.raw,
        model.raw,
        model.name,
        model.raw,
    )
}

fn emit_scalar_enum(model: &ModelSpec, spec: &ScalarEnumModelSpec) -> String {
    let variants = spec.variants.join(",");
    let forward = spec
        .variants
        .iter()
        .map(|variant| format!("{}::{variant} => {}::{variant}", model.name, model.raw))
        .collect::<Vec<_>>()
        .join(",");
    let reverse = spec
        .variants
        .iter()
        .map(|variant| format!("{}::{variant} => Self::{variant}", model.raw))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n#[non_exhaustive]\npub enum {} {{\n{}\n}}\n\nimpl __RustSdkIntoRaw<{}> for {} {{\n    fn into_raw(self) -> {} {{ match self {{\n{}\n    }} }}\n}}\n\nimpl __RustSdkFromRaw<{}> for {} {{\n    fn from_raw(value: {}) -> Self {{ match value {{\n{}\n    }} }}\n}}",
        model.name,
        indent(&variants, 4),
        model.raw,
        model.name,
        model.raw,
        indent(&forward, 8),
        model.raw,
        model.name,
        model.raw,
        indent(&reverse, 8)
    )
}

fn emit_model(model: &ModelSpec) -> String {
    match &model.render {
        ModelRenderSpec::Wrapper(spec) => emit_wrapper(model, spec),
        ModelRenderSpec::Union(spec) => emit_union(model, spec),
        ModelRenderSpec::SimpleUnion(spec) => emit_simple_union(model, spec),
        ModelRenderSpec::View(spec) => emit_view(model, spec),
        ModelRenderSpec::Alias(spec) => format!("pub type {} = {};", model.name, spec.public_type),
        ModelRenderSpec::Map(spec) => emit_map(model, spec),
        ModelRenderSpec::Collection(spec) => emit_collection(model, spec),
        ModelRenderSpec::ScalarEnum(spec) => emit_scalar_enum(model, spec),
    }
}

fn emit_sse_union_public(stream: &StreamPolicy) -> Option<String> {
    if stream.variants.is_empty() {
        return None;
    }
    let variants = stream
        .variants
        .iter()
        .map(|variant| format!("{}({})", variant.name, variant.wrapper))
        .collect::<Vec<_>>()
        .join(",\n");
    Some(format!(
        "#[derive(Debug, Clone)]\n#[non_exhaustive]\npub enum {} {{\n{}\n}}",
        stream.wrapper,
        indent(&variants, 4)
    ))
}

fn emit_sse_union_raw(stream: &StreamPolicy) -> Option<String> {
    if stream.variants.is_empty() {
        return None;
    }
    let variants = stream
        .variants
        .iter()
        .map(|variant| format!("{}({})", variant.name, variant.raw))
        .collect::<Vec<_>>()
        .join(",\n");
    Some(format!(
        "#[derive(Debug, serde::Deserialize)]\n#[serde(untagged)]\nenum {} {{\n{}\n}}",
        stream.item,
        indent(&variants, 4)
    ))
}

fn emit_parameter_constructor_value(field_name: &str, value: Option<&str>) -> String {
    match value {
        None => format!("{field_name}: None"),
        Some(value) if value == field_name => field_name.to_owned(),
        Some(value) => format!("{field_name}: {value}"),
    }
}

fn emit_parameter_request(operation: &OperationSpec) -> String {
    let Some(request) = &operation.parameter_request else {
        return String::new();
    };
    let fields = request
        .fields
        .iter()
        .map(|field| format!("{}: {}", field.name, field.type_name))
        .collect::<Vec<_>>();
    let arguments = request
        .fields
        .iter()
        .filter_map(|field| field.constructor_argument.clone())
        .collect::<Vec<_>>();
    let values = request
        .fields
        .iter()
        .map(|field| {
            emit_parameter_constructor_value(&field.name, field.constructor_value.as_deref())
        })
        .collect::<Vec<_>>();
    let setters = request
        .fields
        .iter()
        .filter_map(|field| {
            Some(format!(
                "#[must_use] pub fn {}(mut self, {}) -> Self {{ self.{} = Some({}); self }}",
                field.name,
                field.setter_argument.as_ref()?,
                field.name,
                field.setter_value.as_ref()?
            ))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let derives = if arguments.is_empty() {
        "Debug, Clone, Default"
    } else {
        "Debug, Clone"
    };
    format!(
        "#[derive({derives})]\npub struct {} {{ {} }}\nimpl {} {{ pub fn new({}) -> Self {{ Self {{ {} }} }}\n{setters}\n}}\n",
        request.name,
        fields.join(", "),
        request.name,
        arguments.join(", "),
        values.join(", ")
    )
}

fn emit_operation_call(
    public_name: &str,
    raw_method: &str,
    call_spec: &OperationCall,
    response: &ResponseProjection,
    runtime: &Runtime,
) -> Result<String> {
    let arguments = &call_spec.arguments;
    let call = &call_spec.raw_arguments;
    let separator = if arguments.is_empty() { "" } else { ", " };
    let error_type = &runtime.error_type;
    Ok(match response {
        ResponseProjection::Sse(stream) if stream.variants.is_empty() => format!(
            "pub async fn {public_name}(&self{separator}{arguments}) -> Result<{}, {error_type}> {{\n    let bytes = self.raw.{raw_method}({call}).await.map_err({error_type}::from)?;\n    let events = {}::{}::<_, _, {}>(bytes)\n        .map(|event| event.map(|event| __RustSdkFromRaw::from_raw(event.data)).map_err(Into::into));\n    Ok(Box::pin(events))\n}}",
            stream.type_name, runtime.sse_module, runtime.sse_function, stream.item
        ),
        ResponseProjection::Sse(stream) => {
            let arms = stream
                .variants
                .iter()
                .map(|variant| {
                    format!(
                        "{}::{}(value) => {}::{}(__RustSdkFromRaw::from_raw(value))",
                        stream.item, variant.name, stream.wrapper, variant.name
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "pub async fn {public_name}(&self{separator}{arguments}) -> Result<{}, {error_type}> {{\n    let bytes = self.raw.{raw_method}({call}).await.map_err({error_type}::from)?;\n    let events = {}::{}::<_, _, {}>(bytes)\n        .map(|event| event.map(|event| match event.data {{ {} }}).map_err(Into::into));\n    Ok(Box::pin(events))\n}}",
                stream.type_name, runtime.sse_module, runtime.sse_function, stream.item, arms
            )
        }
        ResponseProjection::Empty => format!(
            "pub async fn {public_name}(&self{separator}{arguments}) -> Result<(), {error_type}> {{\n    self.raw.{raw_method}({call}).await.map_err(Into::into)\n}}"
        ),
        ResponseProjection::Text => format!(
            "pub async fn {public_name}(&self{separator}{arguments}) -> Result<String, {error_type}> {{\n    self.raw.{raw_method}({call}).await.map_err(Into::into)\n}}"
        ),
        ResponseProjection::BinaryBuffered { type_name } => format!(
            "pub async fn {public_name}(&self{separator}{arguments}) -> Result<{type_name}, {error_type}> {{\n    self.raw.{raw_method}({call}).await.map_err(Into::into)\n}}"
        ),
        ResponseProjection::Binary => format!(
            "pub async fn {public_name}(&self{separator}{arguments}) -> Result<BinaryStream, {error_type}> {{\n    let bytes = self.raw.{raw_method}({call}).await.map_err({error_type}::from)?;\n    let chunks = bytes.map(|chunk| chunk.map_err(Into::into));\n    Ok(Box::pin(chunks))\n}}"
        ),
        ResponseProjection::Json { model, .. } => {
            if let Some(default) = &call_spec.default_raw_arguments {
                let configured = format!(
                    "pub async fn {public_name}_with(&self, {arguments}) -> Result<{model}, {error_type}> {{\n    self.raw.{raw_method}({call}).await.map(__RustSdkFromRaw::from_raw).map_err(Into::into)\n}}"
                );
                format!(
                    "pub async fn {public_name}(&self) -> Result<{model}, {error_type}> {{\n    self.raw.{raw_method}({default}).await.map(__RustSdkFromRaw::from_raw).map_err(Into::into)\n}}\n\n{configured}"
                )
            } else {
                format!(
                    "pub async fn {public_name}(&self{separator}{arguments}) -> Result<{model}, {error_type}> {{\n    self.raw.{raw_method}({call}).await.map(__RustSdkFromRaw::from_raw).map_err(Into::into)\n}}"
                )
            }
        }
    })
}

fn emit_operation(operation: &OperationSpec, runtime: &Runtime) -> Result<String> {
    let primary = emit_operation_call(
        &operation.name,
        &operation.raw_method,
        &operation.call,
        &operation.response_projection,
        runtime,
    )?;
    let Some(filenames) = &operation.multipart_filenames else {
        return Ok(primary);
    };
    let auxiliary = emit_operation_call(
        &format!("{}_with_filenames", operation.name),
        &filenames.raw_method,
        &filenames.call,
        &operation.response_projection,
        runtime,
    )?;
    Ok(format!("{primary}\n\n{auxiliary}"))
}

fn client_name(binding: &BindingLayout) -> &str {
    binding
        .client
        .type_path
        .rsplit("::")
        .next()
        .expect("client path has component")
}

fn emit_resource(
    resource: &ResourceSpec,
    resources: &[ResourceSpec],
    binding: &BindingLayout,
    runtime: &Runtime,
) -> Result<String> {
    let streaming = resource
        .operations
        .iter()
        .any(|operation| matches!(operation.response_projection, ResponseProjection::Sse(_)));
    let binary = resource
        .operations
        .iter()
        .any(|operation| matches!(operation.response_projection, ResponseProjection::Binary));
    let imports = if streaming || binary {
        "use futures_util::StreamExt;\n".to_owned()
    } else {
        String::new()
    };
    let stream_helpers = resource
        .operations
        .iter()
        .filter_map(|operation| {
            let ResponseProjection::Sse(stream) = &operation.response_projection else {
                return None;
            };
            emit_sse_union_raw(stream)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let operations = resource
        .operations
        .iter()
        .map(|operation| emit_operation(operation, runtime))
        .collect::<Result<Vec<_>>>()?
        .join("\n\n");
    let requests = resource
        .operations
        .iter()
        .map(emit_parameter_request)
        .collect::<Vec<_>>()
        .join("\n");
    let mut children: Vec<_> = resources
        .iter()
        .filter(|candidate| {
            candidate.path.len() == resource.path.len() + 1
                && candidate.path[..resource.path.len()] == resource.path
        })
        .collect();
    children.sort_by(|left, right| left.path.cmp(&right.path));
    let child_accessors = children
        .iter()
        .map(|child| {
            format!(
                "pub fn {}(&self) -> {}<'a> {{ {}::new(self.raw) }}",
                child.path.last().expect("child segment"),
                child.name,
                child.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let members = [child_accessors, operations]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let client = client_name(binding);
    Ok(format!(
        "{}{}use super::*;\nuse {};\n{}{}#[derive(Clone, Copy)]\npub struct {}<'a> {{ raw: &'a {client} }}\n\nimpl<'a> {}<'a> {{\n    pub(crate) fn new(raw: &'a {client}) -> Self {{ Self {{ raw }} }}\n{}\n}}\n",
        runtime.generated_marker,
        imports,
        binding.client.type_path,
        if stream_helpers.is_empty() {
            String::new()
        } else {
            format!("{stream_helpers}\n\n")
        },
        requests,
        resource.name,
        resource.name,
        indent(&members, 4)
    ))
}

fn emit_mod(ir: &FacadeIr, binding: &BindingLayout, runtime: &Runtime) -> String {
    let declarations = ir
        .resources
        .iter()
        .map(|resource| {
            format!(
                "{}mod {};",
                if resource.path.len() == 1 { "pub " } else { "" },
                resource.module
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let resources = ir
        .resources
        .iter()
        .map(|resource| format!("pub use {}::{};", resource.module, resource.name))
        .collect::<Vec<_>>()
        .join("\n");
    let mut exported_types: Vec<String> =
        ir.models.iter().map(|model| model.name.clone()).collect();
    exported_types.extend(ir.resources.iter().flat_map(|resource| {
        resource.operations.iter().flat_map(|operation| {
            let ResponseProjection::Sse(stream) = &operation.response_projection else {
                return Vec::new();
            };
            let mut names = vec![stream.type_name.clone()];
            if !stream.variants.is_empty() {
                names.push(stream.wrapper.clone());
            }
            names
        })
    }));
    if ir.resources.iter().any(|resource| {
        resource
            .operations
            .iter()
            .any(|operation| matches!(operation.response_projection, ResponseProjection::Binary))
    }) {
        exported_types.push("BinaryStream".into());
    }
    let accessors = ir
        .resources
        .iter()
        .filter(|resource| resource.path.len() == 1)
        .map(|resource| {
            format!(
                "pub fn {}(&self) -> {}<'_> {{ {}::new(&self.raw) }}",
                resource.path[0], resource.name, resource.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let client = client_name(binding);
    format!(
        "{}{declarations}\npub mod {};\nmod facade_types;\n\n{resources}\npub use {}::{{{}}};\npub use facade_types::{{{}}};\n#[allow(unused_imports, reason = \"generated private transport adapters\")]\npub(crate) use facade_types::{{__RustSdkFromRaw, __RustSdkIntoRaw}};\n\nuse {};\n\n#[derive(Clone)]\npub struct {} {{ raw: {client} }}\n\nimpl {} {{\n    pub fn new(api_key: impl Into<String>) -> Self {{\n        Self {{ raw: {client}::{}().{}(api_key) }}\n    }}\n    #[must_use]\n    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {{\n        self.raw = self.raw.{}(base_url);\n        self\n    }}\n{}\n}}\n",
        runtime.generated_marker,
        runtime.error_module,
        runtime.error_module,
        runtime.error_exports.join(", "),
        exported_types.join(", "),
        binding.client.type_path,
        ir.client_name,
        ir.client_name,
        binding.client.constructor,
        binding.client.api_key_builder,
        binding.client.base_url_builder,
        indent(&accessors, 4)
    )
}

fn emit_facade_types(ir: &FacadeIr, runtime: &Runtime) -> String {
    let mut source = format!(
        "{}use std::pin::Pin;\nuse futures_util::Stream;\nuse super::{};\n\n#[allow(dead_code, reason = \"generated private transport adapter\")]\npub(crate) trait __RustSdkFromRaw<T>: Sized {{ fn from_raw(raw: T) -> Self; }}\n#[allow(dead_code, reason = \"generated private transport adapter\")]\npub(crate) trait __RustSdkIntoRaw<T> {{ fn into_raw(self) -> T; }}\nimpl<T> __RustSdkFromRaw<T> for T {{ fn from_raw(raw: T) -> Self {{ raw }} }}\nimpl<T> __RustSdkIntoRaw<T> for T {{ fn into_raw(self) -> T {{ self }} }}\n",
        runtime.generated_marker, runtime.error_type
    );
    source.push_str(
        &ir.models
            .iter()
            .map(emit_model)
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    let stream_unions = ir
        .resources
        .iter()
        .flat_map(|resource| resource.operations.iter())
        .filter_map(|operation| {
            let ResponseProjection::Sse(stream) = &operation.response_projection else {
                return None;
            };
            emit_sse_union_public(stream)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if !stream_unions.is_empty() {
        source.push_str("\n\n");
        source.push_str(&stream_unions);
    }
    let mut aliases = Vec::new();
    if ir.resources.iter().any(|resource| {
        resource
            .operations
            .iter()
            .any(|operation| matches!(operation.response_projection, ResponseProjection::Binary))
    }) {
        aliases.push(format!(
            "pub type BinaryStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, {}>> + 'static>>;",
            runtime.error_type
        ));
    }
    for resource in &ir.resources {
        for operation in &resource.operations {
            if let ResponseProjection::Sse(stream) = &operation.response_projection {
                aliases.push(format!(
                    "pub type {} = Pin<Box<dyn Stream<Item = Result<{}, {}>> + 'static>>;",
                    stream.type_name, stream.wrapper, runtime.error_type
                ));
            }
        }
    }
    if !aliases.is_empty() {
        source.push_str("\n\n");
        source.push_str(&aliases.join("\n"));
        source.push('\n');
    }
    source
}

pub(crate) fn emit(
    ir: &FacadeIr,
    bindings: &Bindings,
    runtime: &Runtime,
) -> Result<BTreeMap<String, String>> {
    let mut ir = ir.clone();
    qualify_transport_symbols(&mut ir, bindings)?;
    let ir = &ir;
    let binding = &bindings.binding;
    let mut files = BTreeMap::new();
    files.insert("facade_types.rs".into(), emit_facade_types(ir, runtime));
    files.insert("mod.rs".into(), emit_mod(ir, binding, runtime));
    for resource in &ir.resources {
        let filename = format!("{}.rs", resource.module);
        if files.contains_key(&filename) || filename == format!("{}.rs", runtime.error_module) {
            return Err(error(
                "emit.output_collision",
                format!("resource collides with reserved output {filename}"),
            ));
        }
        files.insert(
            filename,
            emit_resource(resource, &ir.resources, binding, runtime)?,
        );
    }
    Ok(files)
}

#[cfg(test)]
mod parameter_request_tests {
    use super::emit_parameter_constructor_value;

    #[test]
    fn constructor_uses_shorthand_only_for_identity_values() {
        assert_eq!(
            emit_parameter_constructor_value("start_time", Some("start_time")),
            "start_time"
        );
        assert_eq!(
            emit_parameter_constructor_value("workspace_id", None),
            "workspace_id: None"
        );
        assert_eq!(
            emit_parameter_constructor_value("name", Some("name.into()")),
            "name: name.into()"
        );
    }
}

#[cfg(test)]
mod optional_ref_tests {
    use super::emit_accessor;
    use crate::AccessorKindDefinition;
    use crate::ir::ResolvedAccessor;

    #[test]
    fn nested_option_borrows_outer_option_without_deref() {
        let mut accessor = ResolvedAccessor {
            name: "trace_id".into(),
            kind: AccessorKindDefinition::OptionalRef,
            path: vec!["trace_id".into()],
            return_type: "Option<&Option<String>>".into(),
            wrapper: None,
            enum_type: None,
            enum_variant: None,
        };
        assert!(emit_accessor(&accessor).contains("self.raw.trace_id.as_ref()"));
        accessor.return_type = "Option<&str>".into();
        assert!(emit_accessor(&accessor).contains("self.raw.trace_id.as_deref()"));
    }
}

#[cfg(test)]
mod private_transport_tests {
    use super::emit;
    use crate::contracts::{Bindings, OpenApi, Runtime, SdkDefinition};
    use crate::lower;

    #[test]
    fn emitted_facade_keeps_backend_transport_interop_private() {
        let openapi = OpenApi(
            serde_json::from_str(include_str!("../tests/fixtures/library/openapi.json")).unwrap(),
        );
        let bindings: Bindings =
            serde_json::from_str(include_str!("../tests/fixtures/library/rust-bindings.json"))
                .unwrap();
        let definition: SdkDefinition =
            serde_json::from_str(include_str!("../tests/fixtures/library/policy.json")).unwrap();
        let ir = lower::lower(&openapi, &bindings, &definition, &Runtime::default()).unwrap();
        let files = emit(&ir, &bindings, &Runtime::default()).unwrap();
        let source = files.values().cloned().collect::<Vec<_>>().join("\n");

        for forbidden in [
            "pub fn raw(&self)",
            "pub fn as_raw(",
            "pub fn into_raw(",
            "pub fn from_raw(",
        ] {
            assert!(
                !source.contains(forbidden),
                "generated public transport escape hatch: {forbidden}"
            );
        }

        for line in source.lines().filter(|line| line.starts_with("impl From<")) {
            for raw in bindings.symbol_paths.keys() {
                assert!(
                    !line.contains(raw),
                    "public From conversion exposes backend symbol {raw}: {line}"
                );
            }
        }

        assert!(source.contains("pub(crate) trait __RustSdkFromRaw"));
        assert!(source.contains("pub(crate) trait __RustSdkIntoRaw"));
        assert!(source.contains("impl<T> __RustSdkFromRaw<T> for T"));
        assert!(source.contains("impl<T> __RustSdkIntoRaw<T> for T"));
        assert!(source.contains("impl __RustSdkFromRaw<crate::generated::types::BookResponse>"));
        assert!(
            source.contains("impl __RustSdkIntoRaw<crate::generated::types::CreateBookRequest>")
        );
    }
}
