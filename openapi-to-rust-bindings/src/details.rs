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
use syn::{
    Expr, FnArg, GenericArgument, ImplItem, Item, Lit, Pat, PathArguments, ReturnType, Stmt, Type,
};

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
    let item_type = canonical_rust_type(&native_parts.0)?;
    let error_type = canonical_rust_type(&native_parts.1)?;
    if item_type != "bytes::Bytes" || error_type != "reqwest::Error" || native_parts.2 != "'static"
    {
        return Err(failure(
            "extract.stream_abi_unproven",
            format!(
                "{path} must be an owned 'static Result<bytes::Bytes, reqwest::Error> stream, got lifetime {}, item {}, error {}",
                native_parts.2, item_type, error_type
            ),
        ));
    }
    Ok(Some(StreamAbiEvidence {
        alias,
        item_type,
        error_type,
        lifetime: native_parts.2,
        native_type: canonical_rust_type(&native.rust_type)?,
        wasm_type: canonical_rust_type(&wasm.rust_type)?,
    }))
}

fn output_is_self(method: &syn::ImplItemFn) -> bool {
    matches!(
        &method.sig.output,
        ReturnType::Type(_, ty) if matches!(&**ty, Type::Path(path) if path.path.is_ident("Self"))
    )
}

fn client_impl_functions<'a>(
    client: &'a syn::File,
    client_type: &str,
) -> Result<BTreeMap<String, &'a syn::ImplItemFn>, Error> {
    let type_name = client_type
        .rsplit("::")
        .next()
        .ok_or_else(|| failure("extract.client_layout_unproven", client_type))?;
    let mut functions = BTreeMap::new();
    for item in &client.items {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() || compact(&tokens(&imp.self_ty)) != type_name {
            continue;
        }
        for item in &imp.items {
            let ImplItem::Fn(method) = item else { continue };
            if !matches!(method.vis, syn::Visibility::Public(_)) {
                continue;
            }
            let name = method.sig.ident.to_string();
            if functions.insert(name.clone(), method).is_some() {
                return Err(failure(
                    "extract.client_layout_unproven",
                    format!("{client_type}: duplicate public method {name}"),
                ));
            }
        }
    }
    Ok(functions)
}

fn tail_expression(block: &syn::Block) -> Option<&Expr> {
    match block.stmts.last()? {
        Stmt::Expr(expr, semicolon) if semicolon.is_none() => Some(expr),
        _ => None,
    }
}

fn constructor_fields(
    method_name: &str,
    method: &syn::ImplItemFn,
    functions: &BTreeMap<String, &syn::ImplItemFn>,
    visiting: &mut BTreeSet<String>,
) -> Result<BTreeSet<String>, Error> {
    if !visiting.insert(method_name.to_owned()) {
        return Err(failure(
            "extract.client_layout_unproven",
            format!("{method_name}: recursive constructor delegation"),
        ));
    }
    let result = (|| {
        let expr = tail_expression(&method.block).ok_or_else(|| {
            failure(
                "extract.client_layout_unproven",
                format!(
                    "{method_name}: constructor does not return a directly provable client state"
                ),
            )
        })?;
        match expr {
            Expr::Struct(value) if value.path.is_ident("Self") => Ok(value
                .fields
                .iter()
                .filter_map(|field| match &field.member {
                    syn::Member::Named(name) => Some(name.to_string()),
                    syn::Member::Unnamed(_) => None,
                })
                .collect()),
            Expr::Call(call) => {
                let Expr::Path(path) = &*call.func else {
                    return Err(failure(
                        "extract.client_layout_unproven",
                        format!("{method_name}: unsupported constructor delegation"),
                    ));
                };
                let segments: Vec<_> = path.path.segments.iter().collect();
                if segments.len() != 2 || segments[0].ident != "Self" {
                    return Err(failure(
                        "extract.client_layout_unproven",
                        format!("{method_name}: constructor delegation must target Self::<method>"),
                    ));
                }
                let delegated = segments[1].ident.to_string();
                let target = functions.get(&delegated).ok_or_else(|| {
                    failure(
                        "extract.client_layout_unproven",
                        format!("{method_name}: delegated constructor {delegated} is not a public client method"),
                    )
                })?;
                if target.sig.receiver().is_some()
                    || !output_is_self(target)
                    || target.sig.asyncness.is_some()
                    || target.sig.inputs.len() != call.args.len()
                {
                    return Err(failure(
                        "extract.client_layout_unproven",
                        format!(
                            "{method_name}: delegated constructor {delegated} has an incompatible signature"
                        ),
                    ));
                }
                constructor_fields(&delegated, target, functions, visiting)
            }
            _ => Err(failure(
                "extract.client_layout_unproven",
                format!("{method_name}: unsupported constructor return expression"),
            )),
        }
    })();
    visiting.remove(method_name);
    result
}

fn self_assignment_field(expr: &Expr) -> Option<&syn::Ident> {
    let Expr::Field(field) = expr else {
        return None;
    };
    let Expr::Path(base) = &*field.base else {
        return None;
    };
    if !base.path.is_ident("self") {
        return None;
    }
    let syn::Member::Named(name) = &field.member else {
        return None;
    };
    Some(name)
}

fn builder_parameter_value(expr: &Expr, parameter: &str) -> bool {
    match expr {
        Expr::Path(path) => path.path.is_ident(parameter),
        Expr::MethodCall(call)
            if call.method == "into"
                && call.args.is_empty()
                && matches!(&*call.receiver, Expr::Path(path) if path.path.is_ident(parameter)) =>
        {
            true
        }
        _ => false,
    }
}

fn api_key_builder_value(expr: &Expr, parameter: &str) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Expr::Path(function) = &*call.func else {
        return false;
    };
    function.path.is_ident("Some")
        && call.args.len() == 1
        && builder_parameter_value(call.args.first().expect("one Some argument"), parameter)
}

struct SelfFieldAssignmentCount<'a> {
    field: &'a str,
    count: usize,
}

impl<'ast> Visit<'ast> for SelfFieldAssignmentCount<'_> {
    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        if self_assignment_field(&node.left).is_some_and(|name| name == self.field) {
            self.count += 1;
        }
        visit::visit_expr_assign(self, node);
    }
}

fn prove_builder(
    client_type: &str,
    method: &syn::ImplItemFn,
    target_field: &str,
    wraps_some: bool,
) -> Result<(), Error> {
    let method_name = method.sig.ident.to_string();
    let Some(receiver) = method.sig.receiver() else {
        return Err(failure(
            "extract.client_layout_unproven",
            format!("{client_type}::{method_name}: expected a by-value self receiver"),
        ));
    };
    if compact(&tokens(receiver)) != "mutself" {
        return Err(failure(
            "extract.client_layout_unproven",
            format!(
                "{client_type}::{method_name}: expected mut self so returned state can be updated"
            ),
        ));
    }
    if method.sig.asyncness.is_some() || !output_is_self(method) {
        return Err(failure(
            "extract.client_layout_unproven",
            format!("{client_type}::{method_name}: expected a synchronous builder returning Self"),
        ));
    }
    let typed: Vec<_> = method
        .sig
        .inputs
        .iter()
        .filter_map(|input| match input {
            FnArg::Typed(value) => Some(value),
            FnArg::Receiver(_) => None,
        })
        .collect();
    if typed.len() != 1 {
        return Err(failure(
            "extract.client_layout_unproven",
            format!("{client_type}::{method_name}: expected exactly one builder argument"),
        ));
    }
    let Pat::Ident(parameter) = &*typed[0].pat else {
        return Err(failure(
            "extract.client_layout_unproven",
            format!("{client_type}::{method_name}: unsupported builder argument pattern"),
        ));
    };
    let parameter_type = compact(&tokens(&typed[0].ty));
    if parameter_type != "implInto<String>" {
        return Err(failure(
            "extract.client_layout_unproven",
            format!(
                "{client_type}::{method_name}: argument type {parameter_type:?} cannot accept the facade's forwarded impl Into<String> value"
            ),
        ));
    }

    let mut all_assignments = SelfFieldAssignmentCount {
        field: target_field,
        count: 0,
    };
    all_assignments.visit_block(&method.block);
    if all_assignments.count != 1 {
        return Err(failure(
            "extract.client_layout_unproven",
            format!(
                "{client_type}::{method_name}: expected exactly one assignment to self.{target_field}, found {}",
                all_assignments.count
            ),
        ));
    }
    let assignments: Vec<_> = method
        .block
        .stmts
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Expr(Expr::Assign(assign), _)
                if self_assignment_field(&assign.left).is_some_and(|name| name == target_field) =>
            {
                Some(assign)
            }
            _ => None,
        })
        .collect();
    if assignments.len() != 1 {
        return Err(failure(
            "extract.client_layout_unproven",
            format!(
                "{client_type}::{method_name}: self.{target_field} assignment is conditional or nested"
            ),
        ));
    }
    let parameter_name = parameter.ident.to_string();
    let correct_value = if wraps_some {
        api_key_builder_value(&assignments[0].right, &parameter_name)
    } else {
        builder_parameter_value(&assignments[0].right, &parameter_name)
    };
    if !correct_value {
        return Err(failure(
            "extract.client_layout_unproven",
            format!(
                "{client_type}::{method_name}: self.{target_field} is not assigned from {parameter_name}"
            ),
        ));
    }
    if !matches!(
        tail_expression(&method.block),
        Some(Expr::Path(path)) if path.path.is_ident("self")
    ) {
        return Err(failure(
            "extract.client_layout_unproven",
            format!("{client_type}::{method_name}: builder does not return the mutated self value"),
        ));
    }
    Ok(())
}

pub(crate) fn prove_client_layout(
    generated: impl AsRef<Path>,
    structural: &StructuralEvidence,
) -> Result<(), Error> {
    let client_path = generated.as_ref().join("client.rs");
    let source = fs::read_to_string(&client_path).map_err(|error| {
        failure(
            "extract.source_unreadable",
            format!("{}: {error}", client_path.display()),
        )
    })?;
    let client = syn::parse_file(&source).map_err(|error| failure("extract.rust_parse", error))?;
    let functions = client_impl_functions(&client, &structural.client.path)?;
    let constructor = functions.get("new").ok_or_else(|| {
        failure(
            "extract.client_layout_unproven",
            format!(
                "{}: public new constructor is missing",
                structural.client.path
            ),
        )
    })?;
    if constructor.sig.receiver().is_some()
        || constructor.sig.asyncness.is_some()
        || !constructor.sig.inputs.is_empty()
        || !output_is_self(constructor)
    {
        return Err(failure(
            "extract.client_layout_unproven",
            format!(
                "{}::new: expected public fn new() -> Self",
                structural.client.path
            ),
        ));
    }
    let fields = constructor_fields("new", constructor, &functions, &mut BTreeSet::new())?;
    for required in ["base_url", "api_key"] {
        if !fields.contains(required) {
            return Err(failure(
                "extract.client_layout_unproven",
                format!(
                    "{}::new: returned client state does not initialize {required}",
                    structural.client.path
                ),
            ));
        }
    }

    let base_url = functions.get("with_base_url").ok_or_else(|| {
        failure(
            "extract.client_layout_unproven",
            format!("{}::with_base_url is missing", structural.client.path),
        )
    })?;
    prove_builder(&structural.client.path, base_url, "base_url", false)?;

    let api_key = functions.get("with_api_key").ok_or_else(|| {
        failure(
            "extract.client_layout_unproven",
            format!("{}::with_api_key is missing", structural.client.path),
        )
    })?;
    prove_builder(&structural.client.path, api_key, "api_key", true)
}

fn pattern_bindings(pat: &Pat, names: &mut Vec<String>) {
    match pat {
        Pat::Ident(value) => names.push(value.ident.to_string()),
        Pat::Tuple(value) => {
            for element in &value.elems {
                pattern_bindings(element, names);
            }
        }
        Pat::TupleStruct(value) => {
            for element in &value.elems {
                pattern_bindings(element, names);
            }
        }
        Pat::Reference(value) => pattern_bindings(&value.pat, names),
        _ => {}
    }
}

fn binding_operand(expr: &Expr, binding: &str) -> bool {
    match expr {
        Expr::Path(path) => path.path.is_ident(binding),
        Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Deref(_)) => {
            binding_operand(&unary.expr, binding)
        }
        Expr::Paren(paren) => binding_operand(&paren.expr, binding),
        Expr::Group(group) => binding_operand(&group.expr, binding),
        _ => false,
    }
}

fn literal_string(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(value) if matches!(&value.lit, Lit::Str(_)) => {
            let Lit::Str(value) = &value.lit else {
                unreachable!()
            };
            Some(value.value())
        }
        _ => None,
    }
}

fn filename_lookup_wire(expr: &Expr) -> Option<String> {
    let Expr::MethodCall(find) = expr else {
        return None;
    };
    if find.method != "find" || find.args.len() != 1 {
        return None;
    }
    let Expr::MethodCall(iter) = &*find.receiver else {
        return None;
    };
    if iter.method != "iter"
        || !iter.args.is_empty()
        || !matches!(&*iter.receiver, Expr::Path(path) if path.path.is_ident("multipart_filenames"))
    {
        return None;
    }
    let Expr::Closure(closure) = find.args.first()? else {
        return None;
    };
    if closure.inputs.len() != 1 {
        return None;
    }
    let mut bindings = Vec::new();
    pattern_bindings(closure.inputs.first()?, &mut bindings);
    if bindings.len() != 1 {
        return None;
    }
    let field = &bindings[0];
    let Expr::Binary(comparison) = &*closure.body else {
        return None;
    };
    if !matches!(comparison.op, syn::BinOp::Eq(_)) {
        return None;
    }
    if binding_operand(&comparison.left, field) {
        literal_string(&comparison.right)
    } else if binding_operand(&comparison.right, field) {
        literal_string(&comparison.left)
    } else {
        None
    }
}

struct IdentUse<'a> {
    ident: &'a str,
    count: usize,
}

impl<'ast> Visit<'ast> for IdentUse<'_> {
    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if node.path.is_ident(self.ident) {
            self.count += 1;
        }
        visit::visit_expr_path(self, node);
    }
}

fn filename_part_evidence(node: &syn::ExprIf) -> Option<(String, String)> {
    let Expr::Let(condition) = &*node.cond else {
        return None;
    };
    let mut filename_bindings = Vec::new();
    pattern_bindings(&condition.pat, &mut filename_bindings);
    if filename_bindings.len() != 1 {
        return None;
    }
    let filename = &filename_bindings[0];
    let wire_name = filename_lookup_wire(&condition.expr)?;

    let Expr::MethodCall(file_name) = tail_expression(&node.then_branch)? else {
        return None;
    };
    if file_name.method != "file_name" || file_name.args.len() != 1 {
        return None;
    }
    let Expr::Path(part) = &*file_name.receiver else {
        return None;
    };
    let part_name = part.path.get_ident()?.to_string();
    let mut filename_use = IdentUse {
        ident: filename,
        count: 0,
    };
    filename_use.visit_expr(file_name.args.first()?);
    if filename_use.count != 1 {
        return None;
    }
    Some((wire_name, part_name))
}

#[derive(Default)]
struct MultipartFilenameSignals {
    filename_parts: BTreeSet<(String, String)>,
    form_parts: BTreeSet<(String, String)>,
}

impl<'ast> Visit<'ast> for MultipartFilenameSignals {
    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        if let Some(evidence) = filename_part_evidence(node) {
            self.filename_parts.insert(evidence);
        }
        visit::visit_expr_if(self, node);
    }

    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        if matches!(&*node.left, Expr::Path(path) if path.path.is_ident("form"))
            && let Expr::MethodCall(call) = &*node.right
            && call.method == "part"
            && matches!(&*call.receiver, Expr::Path(path) if path.path.is_ident("form"))
            && call.args.len() == 2
            && let Some(Expr::Lit(value)) = call.args.first()
            && let Lit::Str(wire) = &value.lit
            && let Some(Expr::Path(part)) = call.args.iter().nth(1)
            && let Some(part) = part.path.get_ident()
        {
            self.form_parts.insert((wire.value(), part.to_string()));
        }
        visit::visit_expr_assign(self, node);
    }
}

fn multipart_kind(
    method: &syn::ImplItemFn,
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
    let mut signals = MultipartFilenameSignals::default();
    signals.visit_block(&method.block);
    if signals.filename_parts.is_empty()
        || !signals
            .filename_parts
            .iter()
            .all(|part| signals.form_parts.contains(part))
    {
        return Err(failure(
            "extract.multipart_helper_unproven",
            format!(
                "{}: multipart_filenames is not proven to select filenames that are applied to emitted multipart form parts",
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

fn signed_integer(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Lit(value) => {
            let Lit::Int(value) = &value.lit else {
                return None;
            };
            value.base10_parse().ok()
        }
        Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Neg(_)) => {
            signed_integer(&unary.expr)?.checked_neg()
        }
        _ => None,
    }
}

fn discriminator_json_value(expr: &Expr) -> Option<Value> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(function) = &*call.func else {
        return None;
    };
    let function = compact(&tokens(&function.path));
    if call.args.len() != 1 {
        return None;
    }
    let argument = call.args.first()?;
    match function.as_str() {
        "serde_json::Value::Bool" => match argument {
            Expr::Lit(value) => {
                let Lit::Bool(value) = &value.lit else {
                    return None;
                };
                Some(Value::Bool(value.value))
            }
            _ => None,
        },
        "serde_json::Value::String" => {
            let Expr::MethodCall(to_string) = argument else {
                return None;
            };
            if to_string.method != "to_string" || !to_string.args.is_empty() {
                return None;
            }
            let Expr::Lit(value) = &*to_string.receiver else {
                return None;
            };
            let Lit::Str(value) = &value.lit else {
                return None;
            };
            Some(Value::String(value.value()))
        }
        "serde_json::Value::Number" => {
            let Expr::Call(number) = argument else {
                return None;
            };
            let Expr::Path(function) = &*number.func else {
                return None;
            };
            if compact(&tokens(&function.path)) != "serde_json::Number::from"
                || number.args.len() != 1
            {
                return None;
            }
            signed_integer(number.args.first()?).map(|value| Value::Number(value.into()))
        }
        _ => None,
    }
}

fn discriminator_initializer(expr: &Expr) -> Option<Value> {
    let Expr::Try(try_expr) = expr else {
        return None;
    };
    let Expr::MethodCall(map_err) = &*try_expr.expr else {
        return None;
    };
    if map_err.method != "map_err" || map_err.args.len() != 1 {
        return None;
    }
    let Expr::Call(from_value) = &*map_err.receiver else {
        return None;
    };
    let Expr::Path(function) = &*from_value.func else {
        return None;
    };
    if compact(&tokens(&function.path)) != "serde_json::from_value" || from_value.args.len() != 1 {
        return None;
    }
    discriminator_json_value(from_value.args.first()?)
}

fn typed_discriminator_local(block: &syn::Block) -> Option<(String, Value)> {
    let mut evidence = None;
    for statement in &block.stmts {
        let Stmt::Local(local) = statement else {
            continue;
        };
        let Pat::Type(typed) = &local.pat else {
            continue;
        };
        let Pat::Ident(ident) = &*typed.pat else {
            continue;
        };
        if ident.ident != "__request_discriminator_value" {
            continue;
        }
        if evidence.is_some() {
            return None;
        }
        let initializer = local.init.as_ref()?;
        let value = discriminator_initializer(&initializer.expr)?;
        evidence = Some((tokens(&typed.ty), value));
    }
    evidence
}

type DiscriminatorAssignment = (String, Value, Vec<String>, usize);

#[derive(Default)]
struct DiscriminatorMarker {
    count: usize,
}

impl<'ast> Visit<'ast> for DiscriminatorMarker {
    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if node.path.is_ident("__request_discriminator_value") {
            self.count += 1;
        }
        visit::visit_expr_path(self, node);
    }
}

fn discriminator_block(block: &syn::Block) -> Result<Option<DiscriminatorAssignment>, Error> {
    let mut marker = DiscriminatorMarker::default();
    marker.visit_block(block);
    let local = typed_discriminator_local(block);
    if local.is_none() && marker.count == 0 {
        return Ok(None);
    }
    let (rust_type, value) = local.ok_or_else(|| {
        failure(
            "extract.request_discriminator_unproven",
            "discriminator marker has no unique direct typed local with a supported serde_json::from_value literal initializer",
        )
    })?;
    let assignments: Vec<_> = block
        .stmts
        .iter()
        .filter_map(|statement| match statement {
            Stmt::Expr(Expr::Assign(assign), _) => {
                let path = field_access(&assign.left)?;
                let depth = discriminator_ref_depth(&assign.right)?;
                Some((path, depth))
            }
            _ => None,
        })
        .collect();
    if assignments.len() != 1 || marker.count != 1 {
        return Err(failure(
            "extract.request_discriminator_unproven",
            format!(
                "discriminator block must contain exactly one direct request assignment and one discriminator value use; found {} assignments and {} uses",
                assignments.len(),
                marker.count
            ),
        ));
    }
    let (path, depth) = assignments.into_iter().next().expect("one assignment");
    Ok(Some((rust_type, value, path, depth)))
}

fn request_rebind(statement: &Stmt) -> bool {
    let Stmt::Local(local) = statement else {
        return false;
    };
    let Pat::Ident(pattern) = &local.pat else {
        return false;
    };
    pattern.ident == "request"
        && pattern.mutability.is_some()
        && local.init.as_ref().is_some_and(
            |init| matches!(&*init.expr, Expr::Path(path) if path.path.is_ident("request")),
        )
}

#[derive(Default)]
struct JsonRequestSerialization {
    count: usize,
}

impl<'ast> Visit<'ast> for JsonRequestSerialization {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Expr::Path(function) = &*node.func
            && compact(&tokens(&function.path)) == "serde_json::to_vec"
            && node.args.len() == 1
            && matches!(
                node.args.first(),
                Some(Expr::Reference(reference))
                    if matches!(&*reference.expr, Expr::Path(path) if path.path.is_ident("request"))
            )
        {
            self.count += 1;
        }
        visit::visit_expr_call(self, node);
    }
}

fn unwrap_option_layers(rust_type: &str, depth: usize) -> Option<String> {
    let mut current: Type = syn::parse_str(rust_type).ok()?;
    for _ in 0..depth {
        let Type::Path(path) = current else {
            return None;
        };
        let segment = path.path.segments.last()?;
        if segment.ident != "Option" {
            return None;
        }
        let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
            return None;
        };
        let GenericArgument::Type(inner) = arguments.args.first()? else {
            return None;
        };
        current = inner.clone();
    }
    Some(tokens(&current))
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
    let mut blocks = Vec::new();
    let mut rebinds = Vec::new();
    let mut serializations = Vec::new();
    for (index, statement) in method.block.stmts.iter().enumerate() {
        if request_rebind(statement) {
            rebinds.push(index);
        }
        let mut serialization = JsonRequestSerialization::default();
        serialization.visit_stmt(statement);
        if serialization.count > 0 {
            for _ in 0..serialization.count {
                serializations.push(index);
            }
        }
        if let Stmt::Expr(Expr::Block(block), _) = statement
            && let Some(evidence) = discriminator_block(&block.block).map_err(|error| {
                failure(
                    "extract.request_discriminator_unproven",
                    format!("{}: {error}", structural_method.name),
                )
            })?
        {
            blocks.push((index, evidence));
            continue;
        }
        let mut marker = DiscriminatorMarker::default();
        marker.visit_stmt(statement);
        if marker.count > 0 {
            return Err(failure(
                "extract.request_discriminator_unproven",
                format!(
                    "{}: discriminator marker occurs outside a direct generated assignment block",
                    structural_method.name
                ),
            ));
        }
    }
    if blocks.is_empty() {
        return Ok(Vec::new());
    }
    if rebinds.len() != 1 || serializations.len() != 1 {
        return Err(failure(
            "extract.request_discriminator_unproven",
            format!(
                "{}: expected one direct mutable request rebind and one JSON serialization, found {} and {}",
                structural_method.name,
                rebinds.len(),
                serializations.len()
            ),
        ));
    }
    let rebind = rebinds[0];
    let serialization = serializations[0];
    if blocks
        .iter()
        .any(|(index, _)| *index <= rebind || *index >= serialization)
    {
        return Err(failure(
            "extract.request_discriminator_unproven",
            format!(
                "{}: discriminator assignments must be unconditional, after request rebinding and before serialization",
                structural_method.name
            ),
        ));
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
    let mut targets = BTreeSet::new();
    for (_, (local_type, value, access_path, assignment_depth)) in blocks {
        if access_path.len() != 1 {
            return Err(failure(
                "extract.request_discriminator_unproven",
                format!(
                    "{} nested discriminator path {:?} requires nested OpenAPI projection proof",
                    structural_method.name, access_path
                ),
            ));
        }
        let target = access_path.join(".");
        if !targets.insert(target.clone()) {
            return Err(failure(
                "extract.request_discriminator_unproven",
                format!(
                    "{} assigns discriminator target {target:?} more than once",
                    structural_method.name
                ),
            ));
        }
        let field = field_for_access(structural, &request_parameter.rust_type, &access_path)
            .map_err(|error| {
                failure(
                    "extract.request_discriminator_unproven",
                    format!("{}: {error}", structural_method.name),
                )
            })?;
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
        let expected_local_type =
            unwrap_option_layers(&field.rust_type, expected_depth).ok_or_else(|| {
                failure(
                    "extract.request_discriminator_unproven",
                    format!(
                        "{} field type {} cannot realize required/nullable/tri-state depth {expected_depth}",
                        structural_method.name, field.rust_type
                    ),
                )
            })?;
        if compact(&local_type) != compact(&expected_local_type) {
            return Err(failure(
                "extract.request_discriminator_unproven",
                format!(
                    "{} discriminator local type {} disagrees with assigned value type {} for field {}",
                    structural_method.name, local_type, expected_local_type, field.rust_type
                ),
            ));
        }
        output.push(RequestDiscriminatorEvidence {
            wire_name,
            rust_access_path: access_path,
            rust_value_type: canonical_rust_type(&local_type)?,
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
            stream_abi(structural, success_type).map_err(|error| {
                failure("extract.stream_abi_unproven", format!("{name}: {error}"))
            })?
        } else {
            None
        };
        output.insert(
            name.clone(),
            OperationDetails {
                kind: multipart_kind(
                    method,
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
