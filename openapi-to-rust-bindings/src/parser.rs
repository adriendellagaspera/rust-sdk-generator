use crate::{Bindings, Error};
use proc_macro2::{LineColumn, Span};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use syn::spanned::Spanned;
use syn::{
    Attribute, Fields, FnArg, GenericArgument, ImplItem, Item, Pat, PathArguments, ReturnType,
    Type, Visibility,
};

struct SourceText<'a> {
    text: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> SourceText<'a> {
    fn new(text: &'a str) -> Self {
        let mut line_starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Self { text, line_starts }
    }

    fn slice<T: Spanned + ?Sized>(&self, node: &T) -> Result<String, Error> {
        self.slice_span(node.span())
    }

    fn slice_span(&self, span: Span) -> Result<String, Error> {
        let start = self.offset(span.start())?;
        let end = self.offset(span.end())?;
        self.text
            .get(start..end)
            .map(ToOwned::to_owned)
            .ok_or_else(|| Error::new("missing Rust AST node"))
    }

    fn offset(&self, position: LineColumn) -> Result<usize, Error> {
        let line = position
            .line
            .checked_sub(1)
            .ok_or_else(|| Error::new("missing Rust AST node"))?;
        self.line_starts
            .get(line)
            .and_then(|start| start.checked_add(position.column))
            .filter(|offset| *offset <= self.text.len())
            .ok_or_else(|| Error::new("missing Rust AST node"))
    }
}

fn parse_file(source: &str, label: &str) -> Result<syn::File, Error> {
    syn::parse_file(source)
        .map_err(|_| Error::new(format!("invalid generated Rust {label} syntax")))
}

fn symbol_name(item: &Item) -> Option<String> {
    match item {
        Item::Struct(item) => Some(item.ident.to_string()),
        Item::Enum(item) => Some(item.ident.to_string()),
        Item::Type(item) => Some(item.ident.to_string()),
        _ => None,
    }
}

fn serde_rename(
    attributes: &[Attribute],
    source: &SourceText<'_>,
) -> Result<Option<String>, Error> {
    let mut matches = Vec::new();
    for attribute in attributes {
        let text = source.slice(attribute)?;
        let Some(body) = text
            .strip_prefix("#[serde(")
            .and_then(|value| value.strip_suffix(")]"))
        else {
            continue;
        };
        let Some((name, raw_value)) = body.split_once('=') else {
            continue;
        };
        if name.trim() != "rename" {
            continue;
        }
        let raw_value = raw_value.trim();
        let Some(value) = raw_value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            continue;
        };
        if value.contains('"') {
            continue;
        }
        matches.push(value.to_owned());
    }
    if matches.len() > 1 {
        return Err(Error::new(
            "multiple serde rename attributes on enum variant",
        ));
    }
    Ok(matches.pop())
}

fn split_generic(value: &str) -> Option<(String, Vec<String>)> {
    let start = value.find('<')?;
    if !value.ends_with('>') {
        return None;
    }
    let constructor = value[..start].trim().to_owned();
    let body = &value[start + 1..value.len() - 1];
    let mut depth = 0_i32;
    let mut part_start = 0;
    let mut parts = Vec::new();
    for (index, character) in body.char_indices() {
        match character {
            '<' | '(' | '[' | '{' => depth += 1,
            '>' | ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(body[part_start..index].trim().to_owned());
                part_start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(body[part_start..].trim().to_owned());
    Some((constructor, parts))
}

fn stream_binding(success_type: &str) -> Value {
    let Some((constructor, arguments)) = split_generic(success_type) else {
        return Value::Null;
    };
    if constructor != "futures_util::stream::BoxStream" || arguments.len() != 2 {
        return Value::Null;
    }
    let Some((event_constructor, event_arguments)) = split_generic(&arguments[1]) else {
        return Value::Null;
    };
    if event_constructor != "Result" || event_arguments.len() != 2 {
        return Value::Null;
    }
    json!({
        "item_type": event_arguments[0],
        "error_type": event_arguments[1],
        "lifetime": arguments[0],
    })
}

fn success_type(return_type: &Type, source: &SourceText<'_>) -> Result<String, Error> {
    let Type::Path(path) = return_type else {
        return Ok(String::new());
    };
    let Some(segment) = path.path.segments.last() else {
        return Ok(String::new());
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Ok(String::new());
    };
    for argument in &arguments.args {
        if let GenericArgument::Type(argument) = argument {
            return source.slice(argument);
        }
    }
    Ok(String::new())
}

fn insert_symbol(
    symbol_paths: &mut BTreeMap<String, String>,
    name: String,
    label: &str,
) -> Result<(), Error> {
    if symbol_paths.contains_key(&name) {
        return Err(Error::new(format!("ambiguous generated symbol {name}")));
    }
    symbol_paths.insert(name.clone(), format!("crate::generated::{label}::{name}"));
    Ok(())
}

/// Parse generated `types.rs` and `client.rs` into normalized Bindings v2.
pub fn parse_bindings(types_source: &str, client_source: &str) -> Result<Bindings, Error> {
    let types_file = parse_file(types_source, "types")?;
    let client_file = parse_file(client_source, "client")?;
    let types_text = SourceText::new(types_source);
    let client_text = SourceText::new(client_source);

    let mut symbol_paths = BTreeMap::new();
    for (label, file) in [("types", &types_file), ("client", &client_file)] {
        for item in &file.items {
            if let Some(name) = symbol_name(item) {
                insert_symbol(&mut symbol_paths, name, label)?;
            }
        }
    }

    let mut structs = Map::new();
    let mut enums = Map::new();
    let mut aliases = Map::new();

    for item in &types_file.items {
        match item {
            Item::Struct(item) => {
                let mut fields = Vec::new();
                if let Fields::Named(named) = &item.fields {
                    for field in &named.named {
                        if !matches!(field.vis, Visibility::Public(_)) {
                            continue;
                        }
                        let name = field
                            .ident
                            .as_ref()
                            .ok_or_else(|| Error::new("missing Rust AST node"))?
                            .to_string();
                        fields.push(json!({
                            "name": name,
                            "type": types_text.slice(&field.ty)?,
                        }));
                    }
                }
                structs.insert(item.ident.to_string(), Value::Array(fields));
            }
            Item::Enum(item) => {
                let mut variants = Vec::new();
                for variant in &item.variants {
                    let payload = match &variant.fields {
                        Fields::Unit => Value::Null,
                        Fields::Unnamed(fields) => {
                            if fields.unnamed.len() != 1 {
                                return Err(Error::new(format!(
                                    "enum {}::{} is not unary",
                                    item.ident, variant.ident
                                )));
                            }
                            Value::String(types_text.slice(&fields.unnamed[0].ty)?)
                        }
                        Fields::Named(_) => Value::Null,
                    };
                    variants.push(json!({
                        "name": variant.ident.to_string(),
                        "payload": payload,
                        "wire_name": serde_rename(&variant.attrs, &types_text)?,
                    }));
                }
                enums.insert(item.ident.to_string(), Value::Array(variants));
            }
            Item::Type(item) => {
                aliases.insert(
                    item.ident.to_string(),
                    Value::String(types_text.slice(&item.ty)?),
                );
            }
            _ => {}
        }
    }

    let mut operations = Map::new();
    let mut found_client = false;
    for item in &client_file.items {
        let Item::Impl(item) = item else {
            continue;
        };
        if client_text.slice(item.self_ty.as_ref())? != "HttpClient" {
            continue;
        }
        found_client = true;
        for item in &item.items {
            let ImplItem::Fn(function) = item else {
                continue;
            };
            let mut parameters = Vec::new();
            for input in &function.sig.inputs {
                let FnArg::Typed(parameter) = input else {
                    continue;
                };
                let Pat::Ident(identifier) = parameter.pat.as_ref() else {
                    return Err(Error::new("missing Rust AST node"));
                };
                parameters.push(json!({
                    "name": identifier.ident.to_string(),
                    "type": client_text.slice(parameter.ty.as_ref())?,
                }));
            }
            let ReturnType::Type(_, return_type) = &function.sig.output else {
                return Err(Error::new("missing Rust AST node"));
            };
            let return_type_text = client_text.slice(return_type.as_ref())?;
            let success_type_text = success_type(return_type.as_ref(), &client_text)?;
            let name = function.sig.ident.to_string();
            operations.insert(
                name.clone(),
                json!({
                    "name": name,
                    "parameters": parameters,
                    "return_type": return_type_text,
                    "success_type": success_type_text,
                    "stream": stream_binding(&success_type_text),
                }),
            );
        }
    }
    if !client_source.trim().is_empty() && !found_client {
        return Err(Error::new("openapi-to-rust HttpClient impl not found"));
    }

    let symbol_paths = symbol_paths
        .into_iter()
        .map(|(name, path)| (name, Value::String(path)))
        .collect::<Map<_, _>>();

    Bindings::from_value(json!({
        "schema_version": 2,
        "structs": structs,
        "enums": enums,
        "aliases": aliases,
        "operations": operations,
        "symbol_paths": symbol_paths,
        "binding": {
            "client": {
                "type_path": "crate::generated::client::HttpClient",
                "constructor": "new",
                "api_key_builder": "with_api_key",
                "base_url_builder": "with_base_url",
            },
            "type_preludes": ["crate::generated::types::*"],
        },
    }))
}
