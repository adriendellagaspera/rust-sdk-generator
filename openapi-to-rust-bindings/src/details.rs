//! Backend-specific semantic details that are only trustworthy when directly
//! observable in emitted Rust.
use crate::Error;
use crate::rust_type::canonical_rust_type;
use crate::semantic::{RepresentationEvidence, SemanticEvidence};
use crate::structural::{AliasEvidence, FieldEvidence, StructuralEvidence};
use quote::ToTokens;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use syn::visit::{self, Visit};
use syn::{Expr, GenericArgument, ImplItem, Item, Lit, PathArguments, Type};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StreamAbiEvidence {
    pub alias: String,
    pub item_type: String,
    pub error_type: String,
    pub lifetime: String,
    pub native_type: String,
    pub wasm_type: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RequestDiscriminatorEvidence {
    pub wire_name: String,
    pub rust_access_path: Vec<String>,
    pub rust_value_type: String,
    pub value: Value,
    pub field_required: bool,
    pub field_nullable: bool,
    pub field_tri_state: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationKindEvidence {
    CallShape,
    MultipartFilenames,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OperationDetails {
    pub kind: OperationKindEvidence,
    pub stream: Option<StreamAbiEvidence>,
    pub request_discriminators: Vec<RequestDiscriminatorEvidence>,
}

fn failure(code: &str, detail: impl std::fmt::Display) -> Error {
    Error::new(format!("{code}: {detail}"))
}

fn tokens<T: ToTokens>(item: &T) -> String {
    item.to_token_stream().to_string()
}

fn compact(value: &str) -> String {
    value.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn cfg_kind(definition: &AliasEvidence) -> Option<&'static str> {
    let cfg = definition
        .cfg
        .iter()
        .map(|item| compact(item))
        .collect::<String>();
    if cfg.contains("not(target_arch=\"wasm32\")") {
        Some("native")
    } else if cfg.contains("target_arch=\"wasm32\"") {
        Some("wasm")
    } else {
        None
    }
}

fn stream_parts(rust_type: &str, outer: &str) -> Result<(String, String, String), Error> {
    let parsed: Type =
        syn::parse_str(rust_type).map_err(|error| failure("extract.stream_abi_unproven", error))?;
    let Type::Path(path) = parsed else {
        return Err(failure("extract.stream_abi_unproven", rust_type));
    };
    let segment = path.path.segments.last().ok_or_else(|| {
        failure(
            "extract.stream_abi_unproven",
            "stream alias type has no path segment",
        )
    })?;
    if segment.ident != outer {
        return Err(failure(
            "extract.stream_abi_unproven",
            format!("expected {outer}, got {}", segment.ident),
        ));
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(failure("extract.stream_abi_unproven", rust_type));
    };
    let mut args = arguments.args.iter();
    let Some(GenericArgument::Lifetime(lifetime)) = args.next() else {
        return Err(failure("extract.stream_abi_unproven", rust_type));
    };
    let Some(GenericArgument::Type(Type::Path(result))) = args.next() else {
        return Err(failure("extract.stream_abi_unproven", rust_type));
    };
    if args.next().is_some() {
        return Err(failure("extract.stream_abi_unproven", rust_type));
    }
    let result_segment = result.path.segments.last().ok_or_else(|| {
        failure(
            "extract.stream_abi_unproven",
            "stream item Result has no segment",
        )
    })?;
    if result_segment.ident != "Result" {
        return Err(failure(
            "extract.stream_abi_unproven",
            format!("stream item is not Result in {rust_type}"),
        ));
    }
    let PathArguments::AngleBracketed(result_args) = &result_segment.arguments else {
        return Err(failure("extract.stream_abi_unproven", rust_type));
    };
    let mut result_args = result_args.args.iter();
    let Some(GenericArgument::Type(item)) = result_args.next() else {
        return Err(failure("extract.stream_abi_unproven", rust_type));
    };
    let Some(GenericArgument::Type(error)) = result_args.next() else {
        return Err(failure("extract.stream_abi_unproven", rust_type));
    };
    if result_args.next().is_some() {
        return Err(failure("extract.stream_abi_unproven", rust_type));
    }
    Ok((tokens(item), tokens(error), tokens(lifetime)))
}

fn stream_abi(
    structural: &StructuralEvidence,
    success_type: &str,
) -> Result<Option<StreamAbiEvidence>, Error> {
    let alias = compact(success_type);
    let path = format!("crate::generated::client::{alias}");
    let Some(definitions) = structural.aliases.get(&path) else {
        return Ok(None);
    };
    if definitions.len() != 2 {
        return Err(failure(
            "extract.stream_abi_unproven",
            format!("{path} must have exactly native and wasm definitions"),
        ));
    }
    let mut native = None;
    let mut wasm = None;
    for definition in definitions {
        match cfg_kind(definition) {
            Some("native") if native.is_none() => native = Some(definition),
            Some("wasm") if wasm.is_none() => wasm = Some(definition),
            _ => {
                return Err(failure(
                    "extract.stream_abi_unproven",
                    format!("{path} has ambiguous cfg variants"),
                ));
            }
        }
    }
    let native =
        native.ok_or_else(|| failure("extract.stream_abi_unproven", "native alias missing"))?;
    let wasm = wasm.ok_or_else(|| failure("extract.stream_abi_unproven", "wasm alias missing"))?;
    let native_parts = stream_parts(&native.rust_type, "BoxStream")?;
    let wasm_parts = stream_parts(&wasm.rust_type, "LocalBoxStream")?;
    if native_parts != wasm_parts {
        return Err(failure(
            "extract.stream_abi_unproven",
            format!("{path} native and wasm item ABI disagree"),
        ));
    }
    Ok(Some(StreamAbiEvidence {
        alias,
        item_type: canonical_rust_type(&native_parts.0)?,
        error_type: canonical_rust_type(&native_parts.1)?,
        lifetime: native_parts.2,
        native_type: canonical_rust_type(&native.rust_type)?,
        wasm_type: canonical_rust_type(&wasm.rust_type)?,
    }))
}

fn multipart_kind(
    structural_method: &crate::structural::MethodEvidence,
    openapi: &Value,
    source_method: &str,
    source_path: &str,
) -> Result<OperationKindEvidence, Error> {
    let matches: Vec<_> = structural_method
        .parameters
        .iter()
        .filter(|parameter| parameter.name == "multipart_filenames")
        .collect();
    if matches.is_empty() {
        return Ok(OperationKindEvidence::CallShape);
    }
    let source_is_multipart = operation_value(openapi, source_method, source_path)?
        .get("requestBody")
        .and_then(|body| body.get("content"))
        .and_then(Value::as_object)
        .is_some_and(|content| content.contains_key("multipart/form-data"));
    if !source_is_multipart
        || matches.len() != 1
        || compact(&matches[0].rust_type) != "&[(&str,&str)]"
    {
        return Err(failure(
            "extract.multipart_helper_unproven",
            format!(
                "{}: multipart filename parameter is not corroborated by the source multipart operation",
                structural_method.name
            ),
        ));
    }
    Ok(OperationKindEvidence::MultipartFilenames)
}

fn field_access(expr: &Expr) -> Option<Vec<String>> {
    fn walk(expr: &Expr, output: &mut Vec<String>) -> bool {
        match expr {
            Expr::Path(path) => path.path.is_ident("request"),
            Expr::Field(field) => {
                if !walk(&field.base, output) {
                    return false;
                }
                let syn::Member::Named(name) = &field.member else {
                    return false;
                };
                output.push(name.to_string());
                true
            }
            _ => false,
        }
    }
    let mut result = Vec::new();
    walk(expr, &mut result).then_some(result)
}

fn discriminator_ref_depth(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::Path(path) if path.path.is_ident("__request_discriminator_value") => Some(0),
        Expr::Call(call)
            if call.func.as_ref().to_token_stream().to_string() == "Some"
                && call.args.len() == 1 =>
        {
            discriminator_ref_depth(call.args.first()?).map(|depth| depth + 1)
        }
        _ => None,
    }
}

#[derive(Default)]
struct AssignmentCollector {
    assignments: Vec<(Vec<String>, usize)>,
}

impl<'ast> Visit<'ast> for AssignmentCollector {
    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        if let (Some(path), Some(depth)) = (
            field_access(&node.left),
            discriminator_ref_depth(&node.right),
        ) {
            self.assignments.push((path, depth));
        }
        visit::visit_expr_assign(self, node);
    }
}

#[derive(Default)]
struct LiteralCollector {
    values: Vec<Value>,
}

impl<'ast> Visit<'ast> for LiteralCollector {
    fn visit_expr_lit(&mut self, node: &'ast syn::ExprLit) {
        match &node.lit {
            Lit::Bool(value) => self.values.push(Value::Bool(value.value)),
            Lit::Int(value) => {
                if let Ok(value) = value.base10_parse::<i64>() {
                    self.values.push(Value::Number(value.into()));
                }
            }
            Lit::Str(value) => self.values.push(Value::String(value.value())),
            _ => {}
        }
        visit::visit_expr_lit(self, node);
    }
}

fn typed_discriminator_local(block: &syn::Block) -> Option<(String, Value)> {
    for statement in &block.stmts {
        let syn::Stmt::Local(local) = statement else {
            continue;
        };
        let syn::Pat::Type(typed) = &local.pat else {
            continue;
        };
        let syn::Pat::Ident(ident) = &*typed.pat else {
            continue;
        };
        if ident.ident != "__request_discriminator_value" {
            continue;
        }
        let initializer = local.init.as_ref()?;
        let mut literals = LiteralCollector::default();
        literals.visit_expr(&initializer.expr);
        let mut values = literals.values;
        values.dedup();
        if values.len() != 1 {
            return None;
        }
        return Some((tokens(&typed.ty), values.remove(0)));
    }
    None
}

struct DiscriminatorBlockVisitor {
    blocks: Vec<(String, Value, Vec<String>, usize)>,
}

impl<'ast> Visit<'ast> for DiscriminatorBlockVisitor {
    fn visit_block(&mut self, block: &'ast syn::Block) {
        if let Some((rust_type, value)) = typed_discriminator_local(block) {
            let mut assignments = AssignmentCollector::default();
            assignments.visit_block(block);
            if assignments.assignments.len() == 1 {
                let (path, depth) = assignments.assignments.remove(0);
                self.blocks.push((rust_type, value, path, depth));
                return;
            }
        }
        visit::visit_block(self, block);
    }
}

fn simple_type_name(rust_type: &str) -> Option<String> {
    let parsed: Type = syn::parse_str(rust_type).ok()?;
    fn walk(ty: &Type) -> Option<String> {
        let Type::Path(path) = ty else { return None };
        let segment = path.path.segments.last()?;
        if matches!(segment.ident.to_string().as_str(), "Option" | "Box") {
            let PathArguments::AngleBracketed(args) = &segment.arguments else {
                return None;
            };
            let GenericArgument::Type(inner) = args.args.first()? else {
                return None;
            };
            walk(inner)
        } else {
            Some(segment.ident.to_string())
        }
    }
    walk(&parsed)
}

fn field_for_access<'a>(
    structural: &'a StructuralEvidence,
    request_type: &str,
    access: &[String],
) -> Result<&'a FieldEvidence, Error> {
    if access.is_empty() {
        return Err(failure(
            "extract.request_discriminator_unproven",
            "empty access path",
        ));
    }
    let mut current = simple_type_name(request_type).ok_or_else(|| {
        failure(
            "extract.request_discriminator_unproven",
            format!("unsupported request type {request_type}"),
        )
    })?;
    for (index, segment) in access.iter().enumerate() {
        let path = format!("crate::generated::types::{current}");
        let model = structural.structs.get(&path).ok_or_else(|| {
            failure(
                "extract.request_discriminator_unproven",
                format!("missing request model {path}"),
            )
        })?;
        let field = model
            .fields
            .iter()
            .find(|field| field.name == *segment)
            .ok_or_else(|| {
                failure(
                    "extract.request_discriminator_unproven",
                    format!("{path} has no field {segment}"),
                )
            })?;
        if index + 1 == access.len() {
            return Ok(field);
        }
        current = simple_type_name(&field.rust_type).ok_or_else(|| {
            failure(
                "extract.request_discriminator_unproven",
                format!("cannot traverse {}", field.rust_type),
            )
        })?;
    }
    unreachable!()
}

fn schema_nullable(schema: &Value) -> bool {
    if schema.get("nullable").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    match schema.get("type") {
        Some(Value::String(value)) if value == "null" => return true,
        Some(Value::Array(values)) if values.iter().any(|value| value.as_str() == Some("null")) => {
            return true;
        }
        _ => {}
    }
    for key in ["anyOf", "oneOf"] {
        if schema
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|values| values.iter().any(schema_nullable))
        {
            return true;
        }
    }
    false
}

fn operation_value<'a>(openapi: &'a Value, method: &str, path: &str) -> Result<&'a Value, Error> {
    openapi
        .get("paths")
        .and_then(|paths| paths.get(path))
        .and_then(|item| item.get(method.to_ascii_lowercase()))
        .ok_or_else(|| {
            failure(
                "extract.request_discriminator_unproven",
                format!("missing source operation {method} {path}"),
            )
        })
}

fn request_schema<'a>(
    openapi: &'a Value,
    operation: &'a Value,
) -> Option<(&'a Value, BTreeSet<String>)> {
    let content = operation.get("requestBody")?.get("content")?.as_object()?;
    let media = content.get("application/json").or_else(|| {
        content
            .iter()
            .find(|(kind, _)| kind.ends_with("+json"))
            .map(|(_, value)| value)
    })?;
    let schema = media.get("schema")?;
    let reference = schema.get("$ref")?.as_str()?;
    let name = reference.rsplit('/').next()?;
    let schema = openapi.get("components")?.get("schemas")?.get(name)?;
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect();
    Some((schema, required))
}

fn request_discriminators(
    method: &syn::ImplItemFn,
    structural: &StructuralEvidence,
    structural_method: &crate::structural::MethodEvidence,
    openapi: &Value,
    source_method: &str,
    source_path: &str,
) -> Result<Vec<RequestDiscriminatorEvidence>, Error> {
    let mut visitor = DiscriminatorBlockVisitor { blocks: Vec::new() };
    visitor.visit_block(&method.block);
    if visitor.blocks.is_empty() {
        return Ok(Vec::new());
    }
    let request_parameter = structural_method
        .parameters
        .iter()
        .find(|parameter| parameter.name == "request")
        .ok_or_else(|| {
            failure(
                "extract.request_discriminator_unproven",
                format!(
                    "{} mutates request but has no request parameter",
                    structural_method.name
                ),
            )
        })?;
    let operation = operation_value(openapi, source_method, source_path)?;
    let (schema, required) = request_schema(openapi, operation).ok_or_else(|| {
        failure(
            "extract.request_discriminator_unproven",
            format!(
                "{} has no named JSON request schema",
                structural_method.name
            ),
        )
    })?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            failure(
                "extract.request_discriminator_unproven",
                "request schema has no properties",
            )
        })?;

    let mut output = Vec::new();
    for (local_type, value, access_path, assignment_depth) in visitor.blocks {
        if access_path.len() != 1 {
            return Err(failure(
                "extract.request_discriminator_unproven",
                format!(
                    "{} nested discriminator path {:?} requires nested OpenAPI projection proof",
                    structural_method.name, access_path
                ),
            ));
        }
        let field = field_for_access(structural, &request_parameter.rust_type, &access_path)?;
        if compact(&local_type) != compact(&field.rust_type) {
            return Err(failure(
                "extract.request_discriminator_unproven",
                format!(
                    "{} discriminator local type {} disagrees with field type {}",
                    structural_method.name, local_type, field.rust_type
                ),
            ));
        }
        let wire_name = field.wire_name.clone().ok_or_else(|| {
            failure(
                "extract.request_discriminator_unproven",
                format!("{} field has no wire name", field.name),
            )
        })?;
        let property = properties.get(&wire_name).ok_or_else(|| {
            failure(
                "extract.request_discriminator_unproven",
                format!("OpenAPI request schema has no property {wire_name}"),
            )
        })?;
        let field_required = required.contains(&wire_name);
        let field_nullable = schema_nullable(property);
        let field_tri_state = compact(&field.rust_type).starts_with("Option<Option<");
        let expected_depth = if field_tri_state {
            2
        } else if !field_required || field_nullable {
            1
        } else {
            0
        };
        if assignment_depth != expected_depth {
            return Err(failure(
                "extract.request_discriminator_unproven",
                format!(
                    "{} assignment depth {assignment_depth} disagrees with field semantics requiring {expected_depth}",
                    structural_method.name
                ),
            ));
        }
        output.push(RequestDiscriminatorEvidence {
            wire_name,
            rust_access_path: access_path,
            rust_value_type: canonical_rust_type(&field.rust_type)?,
            value,
            field_required,
            field_nullable,
            field_tri_state,
        });
    }
    Ok(output)
}

fn client_methods<'a>(
    client: &'a syn::File,
    client_type: &str,
) -> Result<BTreeMap<String, &'a syn::ImplItemFn>, Error> {
    let type_name = client_type
        .rsplit("::")
        .next()
        .ok_or_else(|| failure("extract.client_type_unproven", client_type))?;
    let mut methods = BTreeMap::new();
    for item in &client.items {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() || compact(&tokens(&imp.self_ty)) != type_name {
            continue;
        }
        for item in &imp.items {
            let ImplItem::Fn(method) = item else { continue };
            if matches!(method.vis, syn::Visibility::Public(_))
                && method.sig.asyncness.is_some()
                && method.sig.receiver().is_some()
            {
                methods.insert(method.sig.ident.to_string(), method);
            }
        }
    }
    Ok(methods)
}

pub(crate) fn inspect_details(
    generated: impl AsRef<Path>,
    effective_openapi: impl AsRef<Path>,
    structural: &StructuralEvidence,
    semantic: &SemanticEvidence,
) -> Result<BTreeMap<String, OperationDetails>, Error> {
    let generated = generated.as_ref();
    let client_path = generated.join("client.rs");
    let source = fs::read_to_string(&client_path).map_err(|error| {
        failure(
            "extract.source_unreadable",
            format!("{}: {error}", client_path.display()),
        )
    })?;
    let client = syn::parse_file(&source).map_err(|error| failure("extract.rust_parse", error))?;
    let methods = client_methods(&client, &structural.client.path)?;
    let openapi: Value = serde_json::from_str(
        &fs::read_to_string(effective_openapi.as_ref())
            .map_err(|error| failure("extract.openapi_unreadable", error))?,
    )
    .map_err(|error| failure("extract.openapi_invalid_json", error))?;
    let signatures: BTreeMap<_, _> = structural
        .client
        .methods
        .iter()
        .map(|method| (method.name.as_str(), method))
        .collect();

    let mut output = BTreeMap::new();
    for (name, operation) in &semantic.operations {
        let method = methods
            .get(name)
            .ok_or_else(|| failure("extract.signature_missing", name))?;
        let signature = signatures
            .get(name.as_str())
            .ok_or_else(|| failure("extract.signature_missing", name))?;
        let stream = if matches!(
            operation.representation,
            RepresentationEvidence::EventStream { .. }
                | RepresentationEvidence::BinaryStream { .. }
        ) {
            let success_type = signature.success_type.as_deref().ok_or_else(|| {
                failure(
                    "extract.stream_abi_unproven",
                    format!("{name} has no success type"),
                )
            })?;
            stream_abi(structural, success_type)?
        } else {
            None
        };
        output.insert(
            name.clone(),
            OperationDetails {
                kind: multipart_kind(
                    signature,
                    &openapi,
                    &operation.source_operation.method,
                    &operation.source_operation.path,
                )?,
                stream,
                request_discriminators: request_discriminators(
                    method,
                    structural,
                    signature,
                    &openapi,
                    &operation.source_operation.method,
                    &operation.source_operation.path,
                )?,
            },
        );
    }
    Ok(output)
}
