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
    bounded_text: bool,
    accept_event_stream: bool,
}

fn semantic_error(code: &str, detail: impl std::fmt::Display) -> Error {
    Error::new(format!("{code}: {detail}"))
}

fn normalized_tokens<T: ToTokens>(value: &T) -> String {
    value.to_token_stream().to_string().replace(' ', "")
}

// Both the upstream and fork backends can return text through a shared bounded
// response reader. The successful branch must return the text derived from
// exactly that response, not merely expose a String or declare text in OpenAPI.
fn proves_bounded_text(block: &syn::Block) -> bool {
    let expected = [
        (
            "body_bytes",
            "__read_bounded_response_body(response,self.max_response_body_bytes,).await?",
        ),
        ("raw_body", "body_bytes"),
        (
            "body_text",
            "String::from_utf8_lossy(&raw_body).into_owned()",
        ),
    ];
    let mut next = 0;
    for statement in &block.stmts {
        let syn::Stmt::Local(local) = statement else {
            continue;
        };
        let syn::Pat::Ident(pattern) = &local.pat else {
            continue;
        };
        if !expected.iter().any(|(name, _)| pattern.ident == *name) {
            continue;
        }
        if next == expected.len()
            || pattern.ident != expected[next].0
            || local
                .init
                .as_ref()
                .is_none_or(|init| normalized_tokens(&init.expr) != expected[next].1)
        {
            return false;
        }
        next += 1;
    }
    if next != expected.len() {
        return false;
    }
    let Some(syn::Stmt::Expr(Expr::If(status), _)) = block.stmts.last() else {
        return false;
    };
    matches!(
        status.then_branch.stmts.last(),
        Some(syn::Stmt::Expr(value, None))
            if normalized_tokens(value) == "Ok(body_text)"
    )
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
                let previous = index
                    .checked_sub(1)
                    .and_then(|position| chars.get(position))
                    .copied();
                let next = chars.get(index + 1).copied();
                let boundary = previous
                    .is_some_and(|value| value.is_ascii_lowercase() || value.is_ascii_digit())
                    || (previous.is_some_and(|value| value.is_ascii_uppercase())
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

fn base_rust_method(source: &SourceOperationEvidence, methods: &[String]) -> Result<String, Error> {
    let candidates: Vec<_> = methods
        .iter()
        .filter(|candidate| {
            methods
                .iter()
                .all(|method| method == *candidate || method.starts_with(&format!("{candidate}_")))
        })
        .collect();
    if candidates.len() != 1 {
        return Err(semantic_error(
            "extract.emitted_id_unproven",
            format!(
                "{} {} ({:?}) has no unique emitted base method among {:?}",
                source.method, source.path, source.operation_id, methods
            ),
        ));
    }
    Ok(candidates[0].clone())
}

fn emitted_operation_id(
    source: &SourceOperationEvidence,
    base_method: &str,
    operation_count: usize,
) -> Result<String, Error> {
    let mut candidates = vec![source.operation_id.clone()];
    let method_suffix = source.method.to_ascii_lowercase();
    candidates.push(format!("{}_{}", source.operation_id, method_suffix));
    for suffix in 2..=operation_count.saturating_add(1) {
        candidates.push(format!(
            "{}_{}_{}",
            source.operation_id, method_suffix, suffix
        ));
    }
    let matches: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| conservative_snake_case(candidate).as_deref() == Some(base_method))
        .collect();
    if matches.len() != 1 {
        return Err(semantic_error(
            "extract.emitted_id_unproven",
            format!(
                "{} {} ({:?}) base method {:?} matched emitted-ID candidates {:?}",
                source.method, source.path, source.operation_id, base_method, matches
            ),
        ));
    }
    Ok(matches
        .into_iter()
        .next()
        .expect("one emitted operation id"))
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

fn success_status(status: &str) -> bool {
    let bytes = status.as_bytes();
    bytes.len() == 3
        && bytes[0] == b'2'
        && (bytes[1..].iter().all(u8::is_ascii_digit)
            || bytes[1..].iter().all(|byte| matches!(byte, b'X' | b'x')))
}

fn status_selected(source: &str, emitted: &[String]) -> bool {
    if !success_status(source) {
        return false;
    }
    if emitted.is_empty() {
        return true;
    }
    emitted.iter().any(|candidate| {
        if candidate.eq_ignore_ascii_case("2XX") {
            true
        } else if candidate.len() == 3
            && candidate.as_bytes()[0] == b'2'
            && candidate.as_bytes()[1..].iter().all(u8::is_ascii_digit)
        {
            source == candidate || source.eq_ignore_ascii_case("2XX")
        } else {
            false
        }
    })
}

fn validate_success_statuses(
    operation: &OpenApiOperation,
    method_name: &str,
    emitted: &[String],
) -> Result<(), Error> {
    let responses = operation
        .value
        .get("responses")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            semantic_error(
                "extract.openapi_responses_required",
                format!("{method_name}: {}", operation.identity.operation_id),
            )
        })?;
    let declared: Vec<_> = responses
        .keys()
        .filter(|status| success_status(status))
        .cloned()
        .collect();
    if declared.is_empty() {
        return Err(semantic_error(
            "extract.success_statuses_unproven",
            format!("{method_name}: source operation declares no 2xx response"),
        ));
    }
    if emitted.is_empty() {
        return Ok(());
    }
    for status in emitted {
        let supported = status.eq_ignore_ascii_case("2XX")
            || (status.len() == 3
                && status.as_bytes()[0] == b'2'
                && status.as_bytes()[1..].iter().all(u8::is_ascii_digit));
        if !supported
            || !declared
                .iter()
                .any(|source| status_selected(source, std::slice::from_ref(status)))
        {
            return Err(semantic_error(
                "extract.success_statuses_unproven",
                format!(
                    "{method_name}: emitted success status {status:?} is not covered by source responses {declared:?}"
                ),
            ));
        }
    }
    Ok(())
}

fn success_media(
    operation: &OpenApiOperation,
    emitted_statuses: &[String],
) -> Result<Vec<(String, Value)>, Error> {
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
        if !status_selected(status, emitted_statuses) {
            continue;
        }
        let Some(content) = response.get("content").and_then(Value::as_object) else {
            continue;
        };
        for (media_type, value) in content {
            if !media.iter().any(|(existing_type, existing_value)| {
                existing_type == media_type && existing_value == value
            }) {
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
    success_statuses: &[String],
    signals: &MethodSignals,
) -> Result<RepresentationEvidence, Error> {
    let media = success_media(operation, success_statuses)?;
    let compact = success_type.replace(' ', "");
    if compact == "()" {
        if media.is_empty() {
            return Ok(RepresentationEvidence::Empty);
        }
        return Err(semantic_error(
            "extract.representation_unproven",
            format!(
                "{method_name}: emitted empty success type conflicts with selected source response content"
            ),
        ));
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
    if compact == "String" && (signals.text || signals.bounded_text) && text_media.len() == 1 {
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
        signals.bounded_text = proves_bounded_text(&method.block);
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
        validate_success_statuses(source_operation, &rust_method_name, &success_statuses)?;
        let representation = choose_representation(
            source_operation,
            &rust_method_name,
            success_type,
            &success_statuses,
            &signals,
        )?;
        if matches!(
            representation,
            RepresentationEvidence::EventStream { .. }
                | RepresentationEvidence::BinaryStream { .. }
        ) {
            let alias = success_type
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect::<String>();
            let alias_path = format!("crate::generated::client::{alias}");
            if !structural.aliases.contains_key(&alias_path) {
                unsupported_stream_methods.push(rust_method_name.clone());
            }
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
    let mut source_groups: BTreeMap<SourceOperationEvidence, Vec<String>> = BTreeMap::new();
    for operation in operations.values() {
        source_groups
            .entry(operation.source_operation.clone())
            .or_default()
            .push(operation.rust_method_name.clone());
    }
    for (source_operation, emitted_methods) in source_groups {
        let base_method = base_rust_method(&source_operation, &emitted_methods)?;
        let emitted_id = emitted_operation_id(&source_operation, &base_method, source.len())?;
        for method_name in emitted_methods {
            let operation = operations
                .get_mut(&method_name)
                .expect("grouped operation remains present");
            operation.emitted_operation_id = emitted_id.clone();
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

#[cfg(test)]
mod bounded_text_tests {
    use super::*;

    const EMITTED: &str = r#"{
        let body_bytes = __read_bounded_response_body(
            response,
            self.max_response_body_bytes,
        ).await?;
        let raw_body = body_bytes;
        let body_text = String::from_utf8_lossy(&raw_body).into_owned();
        if status_code == 200 {
            let _ = raw_body;
            Ok(body_text)
        } else {
            Err(ApiOpError::Api(problem))
        }
    }"#;

    fn proved(source: &str) -> bool {
        let block: syn::Block =
            syn::parse_str(source).expect("parse actual generated method block");
        proves_bounded_text(&block)
    }

    #[test]
    fn proves_bounded_text_from_the_response_and_direct_success_return() {
        let block: syn::Block = syn::parse_str(EMITTED).expect("valid generated body");
        let locals = block
            .stmts
            .iter()
            .filter_map(|statement| {
                let syn::Stmt::Local(local) = statement else {
                    return None;
                };
                let syn::Pat::Ident(pattern) = &local.pat else {
                    return None;
                };
                Some((
                    pattern.ident.to_string(),
                    local.init.as_ref().map(|init| normalized_tokens(&init.expr)),
                ))
            })
            .collect::<Vec<_>>();
        assert!(
            proved(EMITTED),
            "unproved bounded text; locals={locals:?}, final={:?}",
            block.stmts.last().map(normalized_tokens),
        );
    }

    #[test]
    fn rejects_unrelated_or_unbounded_text_and_wrong_success_result() {
        for changed in [
            EMITTED.replace("__read_bounded_response_body", "read_other_body"),
            EMITTED.replace("let raw_body = body_bytes;", "let raw_body = other_bytes;"),
            EMITTED.replace(
                "String::from_utf8_lossy(&raw_body)",
                "String::from_utf8_lossy(&other)",
            ),
            EMITTED.replace("Ok(body_text)", "Ok(other_text)"),
            EMITTED.replace(
                "Ok(body_text)",
                "if something { Ok(body_text) } else { Ok(other) }",
            ),
            EMITTED.replace("let raw_body = body_bytes;", "let body_text = body_bytes;"),
        ] {
            assert!(
                !proved(&changed),
                "unproved bounded text was accepted: {changed}"
            );
        }
    }
}
