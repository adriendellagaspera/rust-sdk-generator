//! Structural evidence from ordinary, unmodified openapi-to-rust output.
//!
//! This is NOT a Bindings v3 producer. Source-operation identity, response
//! representation, successful status selection and request discriminators
//! require semantic proof (#150). Never fabricate those fields from names.
use crate::Error;
use quote::ToTokens;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::{Attribute, Fields, FnArg, ImplItem, Item, ReturnType, Type, Visibility};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceLocation {
    pub file: String,
    pub line: usize,
    pub module: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FieldEvidence {
    pub name: String,
    pub rust_type: String,
    pub wire_name: Option<String>,
    pub serde_skip: bool,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StructEvidence {
    pub path: String,
    pub fields: Vec<FieldEvidence>,
    /// Public client/runtime types may contain private implementation fields.
    /// Such a type must not be projected as a complete public model.
    pub has_private_fields: bool,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VariantEvidence {
    pub name: String,
    pub wire_name: String,
    /// Positional payload types. Empty for unit and named-field variants.
    pub payload: Vec<String>,
    /// Exact named variant fields, including error/runtime enums in client.rs.
    pub named_payload: Vec<FieldEvidence>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EnumEvidence {
    pub path: String,
    pub variants: Vec<VariantEvidence>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AliasEvidence {
    pub path: String,
    pub rust_type: String,
    /// Attributes are retained so mutually exclusive cfg variants are not
    /// accidentally collapsed into a made-up cross-target alias.
    pub cfg: Vec<String>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ParameterEvidence {
    pub name: String,
    pub rust_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MethodEvidence {
    pub name: String,
    pub parameters: Vec<ParameterEvidence>,
    pub return_type: String,
    /// Only the syntactically explicit success argument of Rust's Result<T, E>.
    /// Transport semantics and any custom result alias remain unproven.
    pub success_type: Option<String>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ClientEvidence {
    pub path: String,
    pub constructors: Vec<String>,
    pub builders: Vec<String>,
    pub methods: Vec<MethodEvidence>,
    pub imports: Vec<String>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StructuralEvidence {
    /// Fully qualified generated-root-relative Rust paths.
    pub structs: BTreeMap<String, StructEvidence>,
    pub enums: BTreeMap<String, EnumEvidence>,
    /// Multiple declarations for a path are permitted only with explicit cfg
    /// attributes: the exact per-target type is not proven by this inventory.
    pub aliases: BTreeMap<String, Vec<AliasEvidence>>,
    pub client: ClientEvidence,
    /// Kept explicit: no Bindings operation is certified by structure alone.
    pub semantics: &'static str,
}

#[derive(Default)]
struct Inventory {
    structs: BTreeMap<String, StructEvidence>,
    enums: BTreeMap<String, EnumEvidence>,
    aliases: BTreeMap<String, Vec<AliasEvidence>>,
    impls: BTreeMap<String, ImplEvidence>,
    imports: BTreeMap<String, BTreeSet<String>>,
    files: BTreeSet<PathBuf>,
}

#[derive(Default)]
struct ImplEvidence {
    constructors: BTreeSet<String>,
    builders: BTreeSet<String>,
    methods: Vec<MethodEvidence>,
    location: Option<EvidenceLocation>,
}

fn tokens<T: ToTokens>(item: &T) -> String {
    item.to_token_stream().to_string()
}

fn name(ident: &syn::Ident) -> String {
    ident.to_string()
}

fn location(root: &Path, file: &Path, module: &str, line: usize) -> EvidenceLocation {
    EvidenceLocation {
        file: file
            .strip_prefix(root)
            .unwrap_or(file)
            .display()
            .to_string(),
        line,
        module: module.to_owned(),
    }
}

fn failure(loc: &EvidenceLocation, code: &str, detail: impl std::fmt::Display) -> Error {
    Error::new(format!(
        "{code}: {}:{} ({}): {detail}",
        loc.file, loc.line, loc.module
    ))
}

fn attr_cfg(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .map(tokens)
        .collect()
}

/// Extract only a proved serde wire name. Unknown serializer-specific rules
/// are NOT replaced with OpenAPI or presumed Rust naming conventions.
fn serde_name(
    attrs: &[Attribute],
    rust_name: &str,
    loc: &EvidenceLocation,
) -> Result<(Option<String>, bool), Error> {
    let mut renamed: Option<String> = None;
    let mut serialize_rename: Option<String> = None;
    let mut deserialize_rename: Option<String> = None;
    let mut skipped = false;
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                if meta.input.peek(syn::Token![=]) {
                    let value: syn::LitStr = meta.value()?.parse()?;
                    renamed = Some(value.value());
                } else {
                    meta.parse_nested_meta(|nested| {
                        let value: syn::LitStr = nested.value()?.parse()?;
                        if nested.path.is_ident("serialize") {
                            serialize_rename = Some(value.value());
                        } else if nested.path.is_ident("deserialize") {
                            deserialize_rename = Some(value.value());
                        } else {
                            return Err(nested.error("unrecognized serde rename branch"));
                        }
                        Ok(())
                    })?;
                }
            } else if meta.path.is_ident("rename_all") || meta.path.is_ident("flatten") {
                return Err(meta.error("serde rename_all/flatten requires dedicated proof"));
            } else if meta.path.is_ident("skip") {
                skipped = true;
            } else if meta.path.is_ident("skip_serializing")
                || meta.path.is_ident("skip_deserializing")
            {
                return Err(meta.error("directional serde skip requires separate wire projections"));
            } else if meta.input.peek(syn::Token![=]) {
                let _: syn::Expr = meta.value()?.parse()?;
            } else if meta.input.peek(syn::token::Paren) {
                meta.parse_nested_meta(|nested| {
                    if nested.input.peek(syn::Token![=]) {
                        let _: syn::Expr = nested.value()?.parse()?;
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })
        .map_err(|e| failure(loc, "extract.serde_unsupported", e))?;
    }
    let default =
        renamed.unwrap_or_else(|| rust_name.strip_prefix("r#").unwrap_or(rust_name).to_owned());
    let serialization = serialize_rename.unwrap_or_else(|| default.clone());
    let deserialization = deserialize_rename.unwrap_or_else(|| default.clone());
    if serialization != deserialization {
        return Err(failure(
            loc,
            "extract.serde_wire_name_ambiguous",
            format!(
                "serialize and deserialize rename differ: {serialization:?} vs {deserialization:?}"
            ),
        ));
    }
    if skipped {
        Ok((None, true))
    } else {
        Ok((Some(serialization), false))
    }
}

fn reject_unproved_container_serde(
    attrs: &[Attribute],
    loc: &EvidenceLocation,
) -> Result<(), Error> {
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all")
                || meta.path.is_ident("untagged")
                || meta.path.is_ident("tag")
                || meta.path.is_ident("content")
                || meta.path.is_ident("transparent")
                || meta.path.is_ident("from")
                || meta.path.is_ident("into")
                || meta.path.is_ident("remote")
            {
                return Err(meta.error("container serde mapping needs dedicated evidence"));
            }
            if meta.input.peek(syn::Token![=]) {
                let _: syn::Expr = meta.value()?.parse()?;
            } else if meta.input.peek(syn::token::Paren) {
                meta.parse_nested_meta(|nested| {
                    if nested.input.peek(syn::Token![=]) {
                        let _: syn::Expr = nested.value()?.parse()?;
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })
        .map_err(|e| failure(loc, "extract.serde_container_unproven", e))?;
    }
    Ok(())
}

fn untagged_payload_union(
    attrs: &[Attribute],
    variants: &syn::punctuated::Punctuated<syn::Variant, syn::token::Comma>,
) -> bool {
    if !variants.iter().all(|variant| {
        matches!(&variant.fields, Fields::Unnamed(fields) if fields.unnamed.len() == 1)
            && variant
                .attrs
                .iter()
                .all(|attr| !attr.path().is_ident("serde"))
    }) {
        return false;
    }
    let mut saw_untagged = false;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        let parsed = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("untagged") {
                if meta.input.is_empty() && !saw_untagged {
                    saw_untagged = true;
                    return Ok(());
                }
                return Err(meta.error("duplicate or parameterized untagged serde container"));
            }
            Err(meta.error("untagged payload union has unsupported serde container option"))
        });
        if parsed.is_err() {
            return false;
        }
    }
    saw_untagged
}

fn public(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

fn path_of_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) if p.qself.is_none() => Some(tokens(&p.path).replace(' ', "")),
        _ => None,
    }
}

fn self_result(sig: &syn::Signature) -> bool {
    matches!(&sig.output, ReturnType::Type(_, ty) if matches!(&**ty, Type::Path(p) if p.path.is_ident("Self")))
}

fn explicit_result_success(output: &ReturnType) -> Option<String> {
    let ReturnType::Type(_, ty) = output else {
        return None;
    };
    let Type::Path(path) = &**ty else {
        return None;
    };
    let rust_path = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");
    if !matches!(
        rust_path.as_str(),
        "Result" | "std::result::Result" | "core::result::Result"
    ) {
        return None;
    }
    let segment = path.path.segments.last()?;
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    let Some(syn::GenericArgument::Type(success)) = args.args.first() else {
        return None;
    };
    Some(tokens(success))
}

fn inspect_items(
    root: &Path,
    file: &Path,
    module: &str,
    module_dir: &Path,
    items: &[Item],
    state: &mut Inventory,
) -> Result<(), Error> {
    for item in items {
        match item {
            Item::Struct(s) if public(&s.vis) => {
                let path = format!("{module}::{}", name(&s.ident));
                let at = location(root, file, module, s.ident.span().start().line);
                reject_unproved_container_serde(&s.attrs, &at)?;
                let mut fields = Vec::new();
                let mut has_private_fields = false;
                match &s.fields {
                    Fields::Named(named) => {
                        for field in &named.named {
                            let f_at = location(root, file, module, field.span().start().line);
                            if !public(&field.vis) {
                                has_private_fields = true;
                                continue;
                            }
                            let ident = field.ident.as_ref().ok_or_else(|| {
                                failure(&f_at, "extract.field_unidentified", &path)
                            })?;
                            let f_name = name(ident);
                            let (wire_name, serde_skip) = serde_name(&field.attrs, &f_name, &f_at)?;
                            fields.push(FieldEvidence {
                                name: f_name,
                                rust_type: tokens(&field.ty),
                                wire_name,
                                serde_skip,
                                location: f_at,
                            });
                        }
                    }
                    Fields::Unit => {}
                    Fields::Unnamed(_) => {
                        return Err(failure(&at, "extract.tuple_struct_unsupported", &path));
                    }
                }
                if state
                    .structs
                    .insert(
                        path.clone(),
                        StructEvidence {
                            path: path.clone(),
                            fields,
                            has_private_fields,
                            location: at.clone(),
                        },
                    )
                    .is_some()
                {
                    return Err(failure(&at, "extract.duplicate_symbol", path));
                }
            }
            Item::Enum(e) if public(&e.vis) => {
                let path = format!("{module}::{}", name(&e.ident));
                let at = location(root, file, module, e.ident.span().start().line);
                if !untagged_payload_union(&e.attrs, &e.variants) {
                    reject_unproved_container_serde(&e.attrs, &at)?;
                }
                let mut variants = Vec::new();
                for variant in &e.variants {
                    let v_at = location(root, file, module, variant.span().start().line);
                    let v_name = name(&variant.ident);
                    let (wire, skipped) = serde_name(&variant.attrs, &v_name, &v_at)?;
                    if skipped {
                        return Err(failure(&v_at, "extract.enum_variant_skipped", v_name));
                    }
                    let mut named_payload = Vec::new();
                    let payload = match &variant.fields {
                        Fields::Unit => Vec::new(),
                        Fields::Unnamed(f) => f.unnamed.iter().map(|f| tokens(&f.ty)).collect(),
                        Fields::Named(fields) => {
                            for field in &fields.named {
                                let at = location(root, file, module, field.span().start().line);
                                let Some(ident) = field.ident.as_ref() else {
                                    return Err(failure(
                                        &at,
                                        "extract.field_unidentified",
                                        &v_name,
                                    ));
                                };
                                let field_name = name(ident);
                                let (wire_name, serde_skip) =
                                    serde_name(&field.attrs, &field_name, &at)?;
                                named_payload.push(FieldEvidence {
                                    name: field_name,
                                    rust_type: tokens(&field.ty),
                                    wire_name,
                                    serde_skip,
                                    location: at,
                                });
                            }
                            Vec::new()
                        }
                    };
                    variants.push(VariantEvidence {
                        name: v_name,
                        wire_name: wire.unwrap_or_default(),
                        payload,
                        named_payload,
                        location: v_at,
                    });
                }
                if state
                    .enums
                    .insert(
                        path.clone(),
                        EnumEvidence {
                            path: path.clone(),
                            variants,
                            location: at.clone(),
                        },
                    )
                    .is_some()
                {
                    return Err(failure(&at, "extract.duplicate_symbol", path));
                }
            }
            Item::Type(t) if public(&t.vis) => {
                let path = format!("{module}::{}", name(&t.ident));
                let at = location(root, file, module, t.ident.span().start().line);
                let cfg = attr_cfg(&t.attrs);
                let defs = state.aliases.entry(path.clone()).or_default();
                if !defs.is_empty()
                    && (cfg.is_empty() || defs.iter().any(|d| d.cfg.is_empty() || d.cfg == cfg))
                {
                    return Err(failure(
                        &at,
                        "extract.duplicate_alias_without_distinct_cfg",
                        path,
                    ));
                }
                defs.push(AliasEvidence {
                    path,
                    rust_type: tokens(&t.ty),
                    cfg,
                    location: at,
                });
            }
            Item::Use(u) => {
                state
                    .imports
                    .entry(module.to_owned())
                    .or_default()
                    .insert(tokens(&u.tree));
            }
            Item::Impl(imp) if imp.trait_.is_none() => {
                if let Some(ty) = path_of_type(&imp.self_ty) {
                    let path = if ty.contains("::") {
                        ty
                    } else {
                        format!("{module}::{ty}")
                    };
                    let at = location(root, file, module, imp.span().start().line);
                    let entry = state.impls.entry(path).or_default();
                    if entry.location.is_none() {
                        entry.location = Some(at);
                    }
                    for it in &imp.items {
                        let ImplItem::Fn(method) = it else { continue };
                        if !public(&method.vis) {
                            continue;
                        }
                        let method_name = name(&method.sig.ident);
                        let receiver = method
                            .sig
                            .inputs
                            .first()
                            .is_some_and(|a| matches!(a, FnArg::Receiver(_)));
                        if !receiver && self_result(&method.sig) {
                            entry.constructors.insert(method_name);
                            continue;
                        }
                        if receiver && self_result(&method.sig) {
                            entry.builders.insert(method_name);
                            continue;
                        }
                        if !receiver || method.sig.asyncness.is_none() {
                            continue;
                        }
                        let m_at =
                            location(root, file, module, method.sig.ident.span().start().line);
                        let mut parameters = Vec::new();
                        for arg in &method.sig.inputs {
                            match arg {
                                FnArg::Receiver(_) => {}
                                FnArg::Typed(arg) => {
                                    let syn::Pat::Ident(pat) = &*arg.pat else {
                                        return Err(failure(
                                            &m_at,
                                            "extract.parameter_pattern_unsupported",
                                            &method_name,
                                        ));
                                    };
                                    parameters.push(ParameterEvidence {
                                        name: name(&pat.ident),
                                        rust_type: tokens(&arg.ty),
                                    });
                                }
                            }
                        }
                        let return_type = match &method.sig.output {
                            ReturnType::Default => "()".to_owned(),
                            ReturnType::Type(_, ty) => tokens(ty),
                        };
                        entry.methods.push(MethodEvidence {
                            name: method_name,
                            parameters,
                            return_type,
                            success_type: explicit_result_success(&method.sig.output),
                            location: m_at,
                        });
                    }
                }
            }
            Item::Mod(m) => {
                // A public item in a private module is not externally
                // addressable through its declared path. Private helper
                // modules are not public generated SDK symbol evidence.
                if !public(&m.vis) {
                    continue;
                }
                let nested = format!("{module}::{}", name(&m.ident));
                let stem = name(&m.ident);
                // Explicit path attributes change module resolution. Reject
                // them rather than reading an unrelated default-path file.
                if m.attrs.iter().any(|attr| attr.path().is_ident("path")) {
                    return Err(failure(
                        &location(root, file, module, m.ident.span().start().line),
                        "extract.module_path_unsupported",
                        &nested,
                    ));
                }
                let nested_dir = module_dir.join(&stem);
                if let Some((_, items)) = &m.content {
                    inspect_items(root, file, &nested, &nested_dir, items, state)?;
                } else {
                    // Resolve children relative to their containing module,
                    // including when the containing module is inline.
                    let single = module_dir.join(format!("{stem}.rs"));
                    let directory = nested_dir.join("mod.rs");
                    let target = match (single.is_file(), directory.is_file()) {
                        (true, false) => single,
                        (false, true) => directory,
                        _ => {
                            return Err(failure(
                                &location(root, file, module, m.ident.span().start().line),
                                "extract.external_module_unresolved",
                                format!(
                                    "{nested}: expected exactly one of {} or {}",
                                    single.display(),
                                    directory.display()
                                ),
                            ));
                        }
                    };
                    inspect_file(root, &target, &nested, &nested_dir, state)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn inspect_file(
    root: &Path,
    file: &Path,
    module: &str,
    module_dir: &Path,
    state: &mut Inventory,
) -> Result<(), Error> {
    if !state.files.insert(file.to_path_buf()) {
        return Err(Error::new(format!(
            "extract.duplicate_module_file: {}",
            file.display()
        )));
    }
    let source = fs::read_to_string(file).map_err(|e| {
        Error::new(format!(
            "extract.source_unreadable: {}: {e}",
            file.display()
        ))
    })?;
    let ast = syn::parse_file(&source)
        .map_err(|e| Error::new(format!("extract.rust_parse: {}: {e}", file.display())))?;
    inspect_items(root, file, module, module_dir, &ast.items, state)
}

/// Inspect generated raw Rust; deliberately returns structural evidence and
/// never produces/invents canonical Bindings operation metadata.
pub fn inspect_generated(path: impl AsRef<Path>) -> Result<StructuralEvidence, Error> {
    let root = path.as_ref();
    let mut state = Inventory::default();
    for stem in ["types", "client"] {
        let file = root.join(format!("{stem}.rs"));
        if !file.is_file() {
            return Err(Error::new(format!(
                "extract.required_source_missing: {} (ordinary upstream {stem}.rs)",
                file.display()
            )));
        }
        inspect_file(
            root,
            &file,
            &format!("crate::generated::{stem}"),
            &root.join(stem),
            &mut state,
        )?;
    }

    let candidates: Vec<_> = state
        .impls
        .iter()
        .filter(|(_, imp)| !imp.constructors.is_empty() && !imp.methods.is_empty())
        .collect();
    if candidates.len() != 1 {
        return Err(Error::new(format!(
            "extract.client_layout_unproven: expected exactly one public client with a Self-returning constructor and async instance methods; found {}: {:?}",
            candidates.len(),
            candidates
                .iter()
                .map(|(p, _)| p.as_str())
                .collect::<Vec<_>>()
        )));
    }
    let (path, client) = candidates[0];
    if !state.structs.contains_key(path) {
        return Err(Error::new(format!(
            "extract.client_type_unproven: no public struct declaration for {path}"
        )));
    }
    let mut methods = client.methods.clone();
    methods.sort_by(|a, b| a.name.cmp(&b.name));
    for pair in methods.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(failure(
                &pair[1].location,
                "extract.duplicate_client_method",
                &pair[1].name,
            ));
        }
    }
    let module = path.rsplit_once("::").map_or("", |(m, _)| m);
    let imports = state
        .imports
        .get(module)
        .map_or_else(Vec::new, |v| v.iter().cloned().collect());
    Ok(StructuralEvidence {
        structs: state.structs,
        enums: state.enums,
        aliases: state.aliases,
        client: ClientEvidence {
            path: path.clone(),
            constructors: client.constructors.iter().cloned().collect(),
            builders: client.builders.iter().cloned().collect(),
            methods,
            imports,
            location: client
                .location
                .clone()
                .ok_or_else(|| Error::new("extract.client_location_missing"))?,
        },
        semantics: "UNPROVEN: source operation identities, emitted operation IDs, media/transport representations, success statuses and request discriminators require #150",
    })
}
