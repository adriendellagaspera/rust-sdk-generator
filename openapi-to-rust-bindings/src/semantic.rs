//! Semantic evidence recovered from ordinary generated Rust plus the exact
//! effective OpenAPI input. This is backend-specific and fail-closed.
use crate::{Error, EvidenceLocation, inspect_generated};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use syn::spanned::Spanned;
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
    Json { schema_name: String, media_type: String },
    Text { media_type: String },
    BinaryBuffered { media_type: String, wildcard: bool },
    EventStream { media_type: String },
    BinaryStream { media_type: String, wildcard: bool },
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
    status_is_success: bool,
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
    let Expr::Field(field) = expr else { return None };
    let Expr::Path(base) = &*field.base else { return None };
    if !base.path.is_ident("self") {
        return None;
    }
    let syn::Member::Named(name) = &field.member else { return None };
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
            "is_success" => self.status_is_success = true,
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

fn ascii_snake_case(value: &str) -> Option<String> {
    if value.is_empty() || !value.is_ascii() {
        return None;
    }
    let chars: Vec<_> = value.chars().collect();
    let mut out = String::new();
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() {
                let previous = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
                let next = chars.get(index + 1).copied();
                let boundary = previous.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
                    || (previous.is_some_and(|p| p.is_ascii_uppercase())
                        && next.is_some_and(|n| n.is_ascii_lowercase()));
                if boundary && !out.ends_with('_') {
                    out.push('_');
                }
                out.push(ch.to_ascii_lowercase());
            } else {
                out.push(ch);
            }
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    (!out.is_empty()).then_some(out)
}

fn operation_doc(attrs: &[Attribute]) -> Result<(String, String), Error> {
    let mut matches = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("doc")) {
        let Meta::NameValue(meta) = &attr.meta else { continue };
        let Expr::Lit(expr) = &meta.value else { continue };
        let Lit::Str(value) = &expr.lit else { continue };
        let text = value.value();
        let text = text.trim();
        let Some(index) = text.find(char::is_whitespace) else { continue };
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
    if matches.len() != 1 {
        return Err(semantic_error(
            "extract.source_identity_ambiguous",
            format!("expected one HTTP route doc attribute, found {}", matches.len()),
        ));
    }
    Ok(matches.remove(0))
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
        .ok_or_else(|| semantic_error("extract.openapi_paths_required", "paths must be an object"))?;
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
                semantic_error("extract.openapi_operation_invalid", format!("{method} {path}"))
            })?;
            let operation_id = operation
                .get("operationId")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    semantic_error("extract.openapi_operation_id_required", format!("{method} {path}"))
                })?;
            if !ids.insert(operation_id.to_owned()) {
                return Err(semantic_error("extract.openapi_operation_id_duplicate", operation_id));
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
        let success = status.len() == 3
            && status.starts_with('2')
            && status.as_bytes()[1..].iter().all(u8::is_ascii_digit);
        if !success {
            continue;
        }
        let Some(content) = response.get("content").and_then(Value::as_object) else {
            continue;
        };
        for (media_type, value) in content {
            media.push((media_type.clone(), value.clone()));
        }
    }
    Ok(media)
}

fn schema_ref_name(media: &Value) -> Option<String> {
    media.get("schema")
        .and_then(|schema| schema.get("$ref"))
        .and_then(Value::as_str)
        .and_then(|reference| reference.rsplit('/').next())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
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
            if !media.iter().any(|(kind, _)| kind.eq_ignore_ascii_case("text/event-stream")) {
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
            .filter(|(kind, _)| {
                kind.eq_ignore_ascii_case("application/octet-stream")
                    || kind.eq_ignore_ascii_case("application/*")
                    || kind.eq_ignore_ascii_case("*/*")
            })
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
            .filter(|(kind, _)| {
                kind.eq_ignore_ascii_case("application/octet-stream")
                    || kind.eq_ignore_ascii_case("application/*")
                    || kind.eq_ignore_ascii_case("*/*")
            })
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
        let schema_name = schema_ref_name(json[0].1).ok_or_else(|| {
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
        semantic_error("extract.source_unreadable", format!("{}: {e}", client_path.display()))
    })?;
    let client = syn::parse_file(&client_source)
        .map_err(|e| semantic_error("extract.rust_parse", format!("{}: {e}", client_path.display())))?;

    let signatures: BTreeMap<_, _> = structural
        .client
        .methods
        .iter()
        .map(|method| (method.name.as_str(), method))
        .collect();
    let mut operations = BTreeMap::new();
    let mut unsupported_stream_methods = Vec::new();

    for method in client_methods(&client, &structural.client.path)? {
        let rust_method_name = method.sig.ident.to_string();
        let Some(signature) = signatures.get(rust_method_name.as_str()) else {
            return Err(semantic_error(
                "extract.signature_missing",
                format!("{rust_method_name}: no structural signature"),
            ));
        };
        let (verb, path) = operation_doc(&method.attrs)?;
        let source_operation = source.get(&(verb.clone(), path.clone())).ok_or_else(|| {
            semantic_error(
                "extract.source_identity_ambiguous",
                format!("{rust_method_name}: documented {verb} {path} is not an exact OpenAPI operation"),
            )
        })?;
        let mut signals = MethodSignals::default();
        signals.visit_block(&method.block);
        if signals.http_calls != BTreeSet::from([verb.clone()]) {
            return Err(semantic_error(
                "extract.source_identity_ambiguous",
                format!(
                    "{rust_method_name}: documented verb {verb} disagrees with HTTP calls {:?}",
                    signals.http_calls
                ),
            ));
        }
        let skeleton = route_skeleton(&path)?;
        if !signals.string_literals.contains(&skeleton) {
            return Err(semantic_error(
                "extract.source_identity_ambiguous",
                format!("{rust_method_name}: body lacks route skeleton {skeleton:?}"),
            ));
        }
        if !signals.status_is_success {
            return Err(semantic_error(
                "extract.success_statuses_unproven",
                format!("{rust_method_name}: broad 2xx predicate not observed"),
            ));
        }
        let expected_method = ascii_snake_case(&source_operation.identity.operation_id)
            .ok_or_else(|| {
                semantic_error(
                    "extract.emitted_id_unproven",
                    format!("{} cannot be conservatively mapped", source_operation.identity.operation_id),
                )
            })?;
        if expected_method != rust_method_name {
            return Err(semantic_error(
                "extract.emitted_id_unproven",
                format!(
                    "{rust_method_name}: operationId {:?} maps to {expected_method:?}; allocator rename cannot be ruled out",
                    source_operation.identity.operation_id
                ),
            ));
        }
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
            RepresentationEvidence::EventStream { .. } | RepresentationEvidence::BinaryStream { .. }
        ) {
            unsupported_stream_methods.push(rust_method_name.clone());
        }
        let evidence = OperationSemanticEvidence {
            rust_method_name: rust_method_name.clone(),
            source_operation: source_operation.identity.clone(),
            emitted_operation_id: source_operation.identity.operation_id.clone(),
            representation,
            success_statuses: Vec::new(),
            location: EvidenceLocation {
                file: "client.rs".into(),
                line: method.sig.ident.span().start().line,
                module: "crate::generated::client".into(),
            },
        };
        if operations.insert(rust_method_name.clone(), evidence).is_some() {
            return Err(semantic_error("extract.duplicate_client_method", rust_method_name));
        }
    }
    Ok(SemanticEvidence {
        operations,
        unsupported_stream_methods,
    })
}

#[cfg(test)]
mod tests {
    use super::ascii_snake_case;

    #[test]
    fn conservative_ascii_snake_case_matches_common_backend_names() {
        assert_eq!(
            ascii_snake_case("fetchInventoryWithoutNamingShortcut").as_deref(),
            Some("fetch_inventory_without_naming_shortcut")
        );
        assert_eq!(ascii_snake_case("HTTPStatus").as_deref(), Some("http_status"));
        assert_eq!(ascii_snake_case("create-book").as_deref(), Some("create_book"));
        assert_eq!(ascii_snake_case("already_snake").as_deref(), Some("already_snake"));
        assert_eq!(ascii_snake_case(""), None);
        assert_eq!(ascii_snake_case("créate"), None);
    }
}
