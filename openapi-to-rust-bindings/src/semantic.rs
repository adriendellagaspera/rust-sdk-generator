//! Semantic evidence recovered from ordinary generated Rust plus the exact
//! effective OpenAPI input. This is backend-specific and fail-closed.
use crate::structural::EvidenceLocation;
use crate::{Error, inspect_generated};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use syn::visit::{self, Visit};
use syn::{Attribute, Expr, ImplItem, Item, Lit, Meta};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct SourceOperationEvidence {
    pub operation_id: String,
    pub method: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepresentationEvidence {
    Json {
        schema_name: String,
        media_type: String,
    },
    Text {
        media_type: String,
    },
    BinaryBuffered {
        media_type: String,
        wildcard: bool,
    },
    EventStream {
        media_type: String,
    },
    BinaryStream {
        media_type: String,
        wildcard: bool,
    },
    Empty,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OperationSemanticEvidence {
    pub rust_method_name: String,
    pub source_operation: SourceOperationEvidence,
    pub emitted_operation_id: String,
    pub representation: RepresentationEvidence,
    // Empty means the generated code accepts the broad 2xx range.
    pub success_statuses: Vec<String>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticEvidence {
    pub operations: BTreeMap<String, OperationSemanticEvidence>,
    pub unsupported_stream_methods: Vec<String>,
    pub unmatched_source_operations: Vec<SourceOperationEvidence>,
}

#[derive(Clone, Debug)]
struct OpenApiOperation {
    identity: SourceOperationEvidence,
    value: Value,
}

#[derive(Default)]
struct MethodSignals {
    http_calls: BTreeSet<String>,
    string_literals: BTreeSet<String>,
    bytes_stream: bool,
    bytes: bool,
    text: bool,
    accept_event_stream: bool,
}

fn semantic_error(code: &str, detail: impl std::fmt::Display) -> Error {
    Error::new(format!("{code}: {detail}"))
}

fn normalized_tokens<T: ToTokens>(value: &T) -> String {
    value.to_token_stream().to_string().replace(' ', "")
}

fn self_field(expr: &Expr) -> Option<&syn::Ident> {
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

fn literal_strings(tokens: TokenStream, out: &mut BTreeSet<String>) {
    for token in tokens {
        match token {
            TokenTree::Literal(literal) => {
                if let Ok(value) = syn::parse_str::<syn::LitStr>(&literal.to_string()) {
                    out.insert(value.value());
                }
            }
            TokenTree::Group(group) => literal_strings(group.stream(), out),
            _ => {}
        }
    }
}

impl<'ast> Visit<'ast> for MethodSignals {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        if matches!(
            method.as_str(),
            "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "trace"
        ) && self_field(&node.receiver).is_some_and(|field| field == "http_client")
        {
            self.http_calls.insert(method.to_ascii_uppercase());
        }
        match method.as_str() {
            "bytes_stream" => self.bytes_stream = true,
            "bytes" => self.bytes = true,
            "text" => self.text = true,
            "header" => {
                for argument in &node.args {
                    if let Expr::Lit(expr) = argument
                        && let Lit::Str(value) = &expr.lit
                        && value.value().eq_ignore_ascii_case("text/event-stream")
                    {
                        self.accept_event_stream = true;
                    }
                }
            }
            _ => {}
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_lit(&mut self, node: &'ast syn::ExprLit) {
        if let Lit::Str(value) = &node.lit {
            self.string_literals.insert(value.value());
        }
        visit::visit_expr_lit(self, node);
    }

    fn visit_expr_macro(&mut self, node: &'ast syn::ExprMacro) {
        literal_strings(node.mac.tokens.clone(), &mut self.string_literals);
        visit::visit_expr_macro(self, node);
    }
}

fn conservative_snake_case(value: &str) -> Option<String> {
    if value.is_empty() || !value.is_ascii() {
        return None;
    }
    let chars: Vec<_> = value.chars().collect();
    let mut output = String::new();
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() {
                let previous = index.checked_sub(1).and_then(|position| chars.get(position)).copied();
                let next = chars.get(index + 1).copied();
                let boundary = previous.is_some_and(|value| {
                    value.is_ascii_lowercase() || value.is_ascii_digit()
                }) || (previous.is_some_and(|value| value.is_ascii_uppercase())
                    && next.is_some_and(|value| value.is_ascii_lowercase()));
                if boundary && !output.ends_with('_') {
                    output.push('_');
                }
                output.push(ch.to_ascii_lowercase());
            } else {
                output.push(ch);
            }
        } else if !output.is_empty() && !output.ends_with('_') {
            output.push('_');
        }
    }
    while output.ends_with('_') {
        output.pop();
    }
    (!output.is_empty()).then_some(output)
}

fn operation_doc(attrs: &[Attribute]) -> Result<Option<(String, String)>, Error> {
    let mut matches = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("doc")) {
        let Meta::NameValue(meta) = &attr.meta else {
            continue;
        };
        let Expr::Lit(expr) = &meta.value else {
            continue;
        };
        let Lit::Str(value) = &expr.lit else { continue };
        let owned = value.value();
        let text = owned.trim();
        let text = text
            .strip_prefix('`')
            .and_then(|value| value.strip_suffix('`'))
            .unwrap_or(text);
        let Some(index) = text.find(char::is_whitespace) else {
            continue;
        };
        let method = text[..index].trim().to_ascii_uppercase();
        let path = text[index..].trim();
        if matches!(
            method.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE"
        ) && path.starts_with('/')
        {
            matches.push((method, path.to_owned()));
        }
    }
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        count => Err(semantic_error(
            "extract.source_identity_ambiguous",
            format!("multiple HTTP route doc attributes found: {count}"),
        )),
    }
}

fn route_skeleton(path: &str) -> Result<String, Error> {
    let mut output = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '{' {
            output.push(ch);
            continue;
        }
        let mut closed = false;
        for next in chars.by_ref() {
            if next == '}' {
                closed = true;
                break;
            }
        }
        if !closed {
            return Err(semantic_error("extract.source_path_invalid", path));
        }
        output.push_str("{}");
    }
    Ok(output)
}

fn index_openapi(value: &Value) -> Result<BTreeMap<(String, String), OpenApiOperation>, Error> {
    let paths = value
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            semantic_error("extract.openapi_paths_required", "paths must be an object")
        })?;
    let mut operations = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for (path, item) in paths {
        let item = item.as_object().ok_or_else(|| {
            semantic_error("extract.openapi_path_item_invalid", format!("paths.{path}"))
        })?;
        for (verb, operation) in item {
            let method = verb.to_ascii_uppercase();
            if !matches!(
                method.as_str(),
                "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE"
            ) {
                continue;
            }
            let operation = operation.as_object().ok_or_else(|| {
                semantic_error(
                    "extract.openapi_operation_invalid",
                    format!("{method} {path}"),
                )
            })?;
            let operation_id = operation
                .get("operationId")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    semantic_error(
                        "extract.openapi_operation_id_required",
                        format!("{method} {path}"),
                    )
                })?;
            if !ids.insert(operation_id.to_owned()) {
                return Err(semantic_error(
                    "extract.openapi_operation_id_duplicate",
                    operation_id,
                ));
            }
            operations.insert(
                (method.clone(), path.clone()),
                OpenApiOperation {
                    identity: SourceOperationEvidence {
                        operation_id: operation_id.to_owned(),
                        method,
                        path: path.clone(),
                    },
                    value: Value::Object(operation.clone()),
                },
            );
        }
    }
    Ok(operations)
}

#[derive(Default)]
struct StatusCondition {
    broad: bool,
    exact: BTreeSet<String>,
    classes: BTreeSet<String>,
}

fn path_ident(expr: &Expr, ident: &str) -> bool {
    matches!(expr, Expr::Path(path) if path.path.is_ident(ident))
}

impl<'ast> Visit<'ast> for StatusCondition {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "is_success" && path_ident(&node.receiver, "status") {
            self.broad = true;
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if matches!(node.op, syn::BinOp::Eq(_)) {
            let exact = if path_ident(&node.left, "status_code") {
                match &*node.right {
                    Expr::Lit(value) => match &value.lit {
                        Lit::Int(value) => Some(value.base10_digits().to_owned()),
                        _ => None,
                    },
                    _ => None,
                }
            } else if path_ident(&node.right, "status_code") {
                match &*node.left {
                    Expr::Lit(value) => match &value.lit {
                        Lit::Int(value) => Some(value.base10_digits().to_owned()),
                        _ => None,
                    },
                    _ => None,
                }
            } else {
                None
            };
            if let Some(status) = exact {
                self.exact.insert(status);
            }

            let class = |candidate: &Expr, value: &Expr| -> Option<String> {
                let Expr::Binary(div) = candidate else {
                    return None;
                };
                if !matches!(div.op, syn::BinOp::Div(_)) || !path_ident(&div.left, "status_code") {
                    return None;
                }
                let Expr::Lit(divisor) = &*div.right else {
                    return None;
                };
                let Lit::Int(divisor) = &divisor.lit else {
                    return None;
                };
                if divisor.base10_digits() != "100" {
                    return None;
                }
                let Expr::Lit(class) = value else { return None };
                let Lit::Int(class) = &class.lit else {
                    return None;
                };
                let digit = class.base10_digits();
                (digit.len() == 1).then(|| format!("{digit}XX"))
            };
            if let Some(status) =
                class(&node.left, &node.right).or_else(|| class(&node.right, &node.left))
            {
                self.classes.insert(status);
            }
        }
        visit::visit_expr_binary(self, node);
    }
}

fn top_level_status_guard(block: &syn::Block) -> Result<Vec<String>, Error> {
    let mut candidates = Vec::new();
    for statement in &block.stmts {
        let expression = match statement {
            syn::Stmt::Expr(expression, _) => expression,
            _ => continue,
        };
        let Expr::If(branch) = expression else {
            continue;
        };
        let mut condition = StatusCondition::default();
        condition.visit_expr(&branch.cond);
        if condition.broad || !condition.exact.is_empty() || !condition.classes.is_empty() {
            candidates.push(condition);
        }
    }
    if candidates.len() != 1 {
        return Err(semantic_error(
            "extract.success_statuses_unproven",
            format!(
                "expected one top-level generated success guard, found {}",
                candidates.len()
            ),
        ));
    }
    let condition = candidates.remove(0);
    if condition.broad {
        if !condition.exact.is_empty() || !condition.classes.is_empty() {
            return Err(semantic_error(
                "extract.success_statuses_unproven",
                "success guard mixes broad and finite status predicates",
            ));
        }
        return Ok(Vec::new());
    }
    Ok(condition
        .exact
        .into_iter()
        .chain(condition.classes)
        .collect())
}

fn success_media(operation: &OpenApiOperation) -> Result<Vec<(String, Value)>, Error> {
    let responses = operation
        .value
        .get("responses")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            semantic_error(
                "extract.openapi_responses_required",
                &operation.identity.operation_id,
            )
        })?;
    let mut media = Vec::new();
    for (status, response) in responses {
        let bytes = status.as_bytes();
        let success = bytes.len() == 3
            && bytes[0] == b'2'
            && (bytes[1..].iter().all(u8::is_ascii_digit)
                || bytes[1..].iter().all(|byte| matches!(byte, b'X' | b'x')));
        if !success {
            continue;
        }
        let Some(content) = response.get("content").and_then(Value::as_object) else {
            continue;
        };
        for (media_type, value) in content {
            if !media
                .iter()
                .any(|(existing_type, existing_value)| {
                    existing_type == media_type && existing_value == value
                })
            {
                media.push((media_type.clone(), value.clone()));
            }
        }
    }
    Ok(media)
}

fn schema_ref_name(media: &Value) -> Option<String> {
    media
        .get("schema")
        .and_then(|schema| schema.get("$ref"))
        .and_then(Value::as_str)
        .and_then(|reference| reference.rsplit('/').next())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn is_binary_media(media_type: &str, media: &Value) -> bool {
    media_type.eq_ignore_ascii_case("application/octet-stream")
        || media_type.eq_ignore_ascii_case("application/*")
        || media_type.eq_ignore_ascii_case("*/*")
        || media
            .get("schema")
            .and_then(|schema| schema.get("format"))
            .and_then(Value::as_str)
            .is_some_and(|format| format.eq_ignore_ascii_case("binary"))
}

fn choose_representation(
    operation: &OpenApiOperation,
    method_name: &str,
    success_type: &str,
    signals: &MethodSignals,
) -> Result<RepresentationEvidence, Error> {
    let media = success_media(operation)?;
    let compact = success_type.replace(' ', "");
    if compact == "()" {
        return Ok(RepresentationEvidence::Empty);
    }
    let streaming = signals.bytes_stream
        || compact.contains("Stream<")
        || compact.contains("BoxStream<")
        || compact.contains("LocalBoxStream<");
    if streaming {
        if signals.accept_event_stream {
            if !media
                .iter()
                .any(|(kind, _)| kind.eq_ignore_ascii_case("text/event-stream"))
            {
                return Err(semantic_error(
                    "extract.representation_unproven",
                    format!("{method_name}: emitted SSE Accept has no matching OpenAPI media type"),
                ));
            }
            return Ok(RepresentationEvidence::EventStream {
                media_type: "text/event-stream".into(),
            });
        }
        let binary: Vec<_> = media
            .iter()
            .filter(|(kind, media)| is_binary_media(kind, media))
            .collect();
        if binary.len() != 1 {
            return Err(semantic_error(
                "extract.representation_unproven",
                format!("{method_name}: streamed binary media is ambiguous"),
            ));
        }
        return Ok(RepresentationEvidence::BinaryStream {
            media_type: binary[0].0.clone(),
            wildcard: binary[0].0.contains('*'),
        });
    }
    if signals.bytes || compact == "bytes::Bytes" {
        let binary: Vec<_> = media
            .iter()
            .filter(|(kind, media)| is_binary_media(kind, media))
            .collect();
        if binary.len() != 1 {
            return Err(semantic_error(
                "extract.representation_unproven",
                format!("{method_name}: buffered binary media is ambiguous"),
            ));
        }
        return Ok(RepresentationEvidence::BinaryBuffered {
            media_type: binary[0].0.clone(),
            wildcard: binary[0].0.contains('*'),
        });
    }
    let json: Vec<_> = media
        .iter()
        .filter(|(kind, _)| {
            kind.eq_ignore_ascii_case("application/json")
                || kind.to_ascii_lowercase().ends_with("+json")
        })
        .collect();
    if !json.is_empty() {
        if json.len() != 1 {
            return Err(semantic_error(
                "extract.representation_unproven",
                format!("{method_name}: JSON media is ambiguous"),
            ));
        }
        let schema_name = schema_ref_name(&json[0].1).ok_or_else(|| {
            semantic_error(
                "extract.response_schema_unproven",
                format!("{method_name}: JSON success schema is not a named ref"),
            )
        })?;
        if compact != schema_name.replace(' ', "") {
            return Err(semantic_error(
                "extract.response_schema_unproven",
                format!(
                    "{method_name}: generated success type {success_type:?} does not match schema {schema_name:?}"
                ),
            ));
        }
        return Ok(RepresentationEvidence::Json {
            schema_name,
            media_type: json[0].0.clone(),
        });
    }
    let text_media: Vec<_> = media
        .iter()
        .filter(|(kind, _)| kind.to_ascii_lowercase().starts_with("text/"))
        .collect();
    if compact == "String" && signals.text && text_media.len() == 1 {
        return Ok(RepresentationEvidence::Text {
            media_type: text_media[0].0.clone(),
        });
    }
    Err(semantic_error(
        "extract.representation_unproven",
        format!("{method_name}: emitted behavior does not prove one declared representation"),
    ))
}

fn client_methods<'a>(
    client: &'a syn::File,
    client_type: &str,
) -> Result<Vec<&'a syn::ImplItemFn>, Error> {
    let type_name = client_type
        .rsplit("::")
        .next()
        .ok_or_else(|| semantic_error("extract.client_type_unproven", client_type))?;
    let mut methods = Vec::new();
    for item in &client.items {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() || normalized_tokens(&imp.self_ty) != type_name {
            continue;
        }
        for item in &imp.items {
            let ImplItem::Fn(method) = item else { continue };
            if matches!(method.vis, syn::Visibility::Public(_))
                && method.sig.asyncness.is_some()
                && method.sig.receiver().is_some()
            {
                methods.push(method);
            }
        }
    }
    Ok(methods)
}

pub fn inspect_semantics(
    generated: impl AsRef<Path>,
    effective_openapi: impl AsRef<Path>,
) -> Result<SemanticEvidence, Error> {
    let generated = generated.as_ref();
    let structural = inspect_generated(generated)?;
    let openapi_source = fs::read_to_string(effective_openapi.as_ref()).map_err(|e| {
        semantic_error(
            "extract.openapi_unreadable",
            format!("{}: {e}", effective_openapi.as_ref().display()),
        )
    })?;
    let openapi: Value = serde_json::from_str(&openapi_source)
        .map_err(|e| semantic_error("extract.openapi_invalid_json", e))?;
    let source = index_openapi(&openapi)?;
    let client_path = generated.join("client.rs");
    let client_source = fs::read_to_string(&client_path).map_err(|e| {
        semantic_error(
            "extract.source_unreadable",
            format!("{}: {e}", client_path.display()),
        )
    })?;
    let client = syn::parse_file(&client_source).map_err(|e| {
        semantic_error(
            "extract.rust_parse",
            format!("{}: {e}", client_path.display()),
        )
    })?;

    let signatures: BTreeMap<_, _> = structural
        .client
        .methods
        .iter()
        .map(|method| (method.name.as_str(), method))
        .collect();
    let mut operations = BTreeMap::new();
    let mut unsupported_stream_methods = Vec::new();
    let mut matched_sources = BTreeSet::new();

    for method in client_methods(&client, &structural.client.path)? {
        let rust_method_name = method.sig.ident.to_string();
        let Some(signature) = signatures.get(rust_method_name.as_str()) else {
            return Err(semantic_error(
                "extract.signature_missing",
                format!("{rust_method_name}: no structural signature"),
            ));
        };
        let documented = operation_doc(&method.attrs)?;
        let mut signals = MethodSignals::default();
        signals.visit_block(&method.block);
        if signals.http_calls.len() != 1 {
            return Err(semantic_error(
                "extract.source_identity_ambiguous",
                format!(
                    "{rust_method_name}: expected one observable HTTP verb, found {:?}",
                    signals.http_calls
                ),
            ));
        }
        let verb = signals
            .http_calls
            .iter()
            .next()
            .cloned()
            .expect("one HTTP verb");
        let mut candidates = Vec::new();
        for ((candidate_verb, candidate_path), operation) in &source {
            if candidate_verb != &verb {
                continue;
            }
            let skeleton = route_skeleton(candidate_path)?;
            if signals.string_literals.contains(&skeleton) {
                candidates.push(operation);
            }
        }
        if candidates.len() != 1 {
            return Err(semantic_error(
                "extract.source_identity_ambiguous",
                format!(
                    "{rust_method_name}: {verb} body route evidence matched {} OpenAPI operations",
                    candidates.len()
                ),
            ));
        }
        let source_operation = candidates.remove(0);
        if let Some((doc_verb, doc_path)) = documented
            && (doc_verb != source_operation.identity.method
                || route_skeleton(&doc_path)? != route_skeleton(&source_operation.identity.path)?)
        {
            return Err(semantic_error(
                "extract.source_identity_ambiguous",
                format!(
                    "{rust_method_name}: rustdoc {doc_verb} {doc_path} disagrees with body/OpenAPI identity {} {}",
                    source_operation.identity.method, source_operation.identity.path
                ),
            ));
        }
        matched_sources.insert((
            source_operation.identity.method.clone(),
            source_operation.identity.path.clone(),
        ));
        let success_statuses = top_level_status_guard(&method.block).map_err(|error| {
            semantic_error(
                "extract.success_statuses_unproven",
                format!("{rust_method_name}: {error}"),
            )
        })?;
        let success_type = signature.success_type.as_deref().ok_or_else(|| {
            semantic_error(
                "extract.success_type_unproven",
                format!("{rust_method_name}: return type is not explicit Result<T, E>"),
            )
        })?;
        let representation =
            choose_representation(source_operation, &rust_method_name, success_type, &signals)?;
        if matches!(
            representation,
            RepresentationEvidence::EventStream { .. }
                | RepresentationEvidence::BinaryStream { .. }
        ) {
            unsupported_stream_methods.push(rust_method_name.clone());
        }
        let evidence = OperationSemanticEvidence {
            rust_method_name: rust_method_name.clone(),
            source_operation: source_operation.identity.clone(),
            emitted_operation_id: source_operation.identity.operation_id.clone(),
            representation,
            success_statuses,
            location: EvidenceLocation {
                file: "client.rs".into(),
                line: method.sig.ident.span().start().line,
                module: "crate::generated::client".into(),
            },
        };
        if operations
            .insert(rust_method_name.clone(), evidence)
            .is_some()
        {
            return Err(semantic_error(
                "extract.duplicate_client_method",
                rust_method_name,
            ));
        }
    }
    let mut source_groups: BTreeMap<SourceOperationEvidence, Vec<&OperationSemanticEvidence>> =
        BTreeMap::new();
    for operation in operations.values() {
        source_groups
            .entry(operation.source_operation.clone())
            .or_default()
            .push(operation);
    }
    for (source_operation, emitted) in source_groups {
        let expected_base = conservative_snake_case(&source_operation.operation_id).ok_or_else(|| {
            semantic_error(
                "extract.emitted_id_unproven",
                format!(
                    "source operationId {:?} cannot be conservatively mapped to the backend's ordinary Rust method name",
                    source_operation.operation_id
                ),
            )
        })?;
        if !emitted
            .iter()
            .any(|operation| operation.rust_method_name == expected_base)
        {
            return Err(semantic_error(
                "extract.emitted_id_unproven",
                format!(
                    "{} {} ({:?}) has no observed base method {:?}; analyzer renaming cannot be ruled out",
                    source_operation.method,
                    source_operation.path,
                    source_operation.operation_id,
                    expected_base,
                ),
            ));
        }
    }

    let unmatched_source_operations = source
        .iter()
        .filter(|(key, _)| !matched_sources.contains(*key))
        .map(|(_, operation)| operation.identity.clone())
        .collect();
    Ok(SemanticEvidence {
        operations,
        unsupported_stream_methods,
        unmatched_source_operations,
    })
}
