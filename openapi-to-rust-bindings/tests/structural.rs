use openapi_to_rust_bindings::inspect_generated;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bindings-structural-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create fixture directory");
        Self(dir)
    }
    fn file(&self, path: &str, content: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, content).expect("write source");
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const CLIENT: &str = r#"
use super::types::*;
pub struct RawClient { base_url: String }
impl RawClient {
    pub fn new() -> Self { Self { base_url: String::new() } }
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
    pub async fn visit(&self, id: impl AsRef<str>, offset: Option<u32>, body: inner::Model)
        -> Result<Vec<inner::Model>, ApiError>
    { todo!() }
    pub async fn visit_binary(&self) -> Result<bytes::Bytes, ApiError> { todo!() }
}
pub enum ParameterOrder {
    #[serde(rename = "desc")]
    Descending,
    Ascending,
}
pub enum ApiOpError {
    ResponseTooLarge { limit: usize, received: usize },
    Other,
}
#[cfg(not(target_arch = "wasm32"))]
pub type ResponseStream = futures_util::stream::BoxStream<'static, Result<bytes::Bytes, Error>>;
#[cfg(target_arch = "wasm32")]
pub type ResponseStream = futures_util::stream::LocalBoxStream<'static, Result<bytes::Bytes, Error>>;
"#;

const TYPES: &str = r#"
pub mod inner {
    #[derive(Clone)]
    pub struct Model {
        #[serde(rename = "wire-name")]
        pub r#type: Option<String>,
        #[serde(skip)]
        pub computed: bool,
    }
    pub enum Kind {
        #[serde(rename = "walk-fast")]
        Fast(String),
        Slow,
    }
    pub type Models = Vec<Model>;
}
"#;

#[test]
fn source_ast_inventory_proves_models_signatures_paths_and_cfg_aliases_without_manifest() {
    let root = Scratch::new();
    root.file("types.rs", TYPES);
    root.file("client.rs", CLIENT);
    let first = inspect_generated(root.path()).expect("inspect ordinary Rust");
    assert_eq!(
        first,
        inspect_generated(root.path()).expect("deterministic extraction")
    );
    let model = &first.structs["crate::generated::types::inner::Model"];
    assert_eq!(model.fields[0].name, "r#type");
    assert_eq!(model.fields[0].wire_name.as_deref(), Some("wire-name"));
    assert_eq!(model.fields[1].wire_name, None);
    assert!(model.fields[1].serde_skip);
    assert_eq!(model.location.file, "types.rs");
    assert!(model.location.line > 0);
    let kind = &first.enums["crate::generated::types::inner::Kind"];
    assert_eq!(kind.variants[0].wire_name, "walk-fast");
    assert_eq!(kind.variants[0].payload, vec!["String"]);
    assert_eq!(
        first.aliases["crate::generated::types::inner::Models"][0].rust_type,
        "Vec < Model >"
    );
    assert_eq!(
        first.aliases["crate::generated::client::ResponseStream"].len(),
        2
    );
    assert!(
        first.aliases["crate::generated::client::ResponseStream"]
            .iter()
            .all(|a| !a.cfg.is_empty())
    );
    assert!(
        first
            .enums
            .contains_key("crate::generated::client::ParameterOrder")
    );
    let error_enum = &first.enums["crate::generated::client::ApiOpError"];
    assert_eq!(error_enum.variants[0].named_payload.len(), 2);
    assert_eq!(error_enum.variants[0].named_payload[0].name, "limit");
    assert!(first.structs["crate::generated::client::RawClient"].has_private_fields);
    assert_eq!(first.client.path, "crate::generated::client::RawClient");
    assert_eq!(first.client.constructors, vec!["new"]);
    assert_eq!(first.client.builders, vec!["with_base_url"]);
    assert_eq!(first.client.methods.len(), 2);
    let visit = first
        .client
        .methods
        .iter()
        .find(|m| m.name == "visit")
        .expect("visit");
    assert_eq!(
        visit
            .parameters
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "offset", "body"]
    );
    assert!(visit.parameters[0].rust_type.contains("AsRef"));
    assert!(visit.return_type.contains("Result"));
    assert_eq!(
        visit.success_type.as_deref(),
        Some("Vec < inner :: Model >")
    );
    assert!(
        first
            .client
            .imports
            .iter()
            .any(|u| u.contains("super") && u.contains("types"))
    );
    assert!(first.semantics.contains("UNPROVEN"));
    assert!(!root.path().join("binding-manifest.json").exists());
}

#[test]
fn external_nested_modules_resolve_with_real_source_location() {
    let root = Scratch::new();
    root.file("types.rs", "pub mod nested;\n");
    root.file("types/nested.rs", "pub struct Part { pub id: String }\n");
    root.file("client.rs", CLIENT);
    let observed = inspect_generated(root.path()).expect("external nested module");
    let model = &observed.structs["crate::generated::types::nested::Part"];
    assert_eq!(model.location.file, "types/nested.rs");
    assert_eq!(model.fields[0].wire_name.as_deref(), Some("id"));
}

#[test]
fn private_modules_do_not_expose_inaccessible_public_items() {
    let root = Scratch::new();
    root.file(
        "types.rs",
        "pub struct Visible { pub id: String }\nmod internal { pub struct Hidden { pub id: String } }\n",
    );
    root.file("client.rs", CLIENT);
    let evidence = inspect_generated(root.path()).expect("public symbols only");
    assert!(
        evidence
            .structs
            .contains_key("crate::generated::types::Visible")
    );
    assert!(
        !evidence
            .structs
            .contains_key("crate::generated::types::internal::Hidden")
    );
}

#[test]
fn nested_inline_module_resolves_external_child_from_its_own_directory() {
    let root = Scratch::new();
    root.file("types.rs", "pub mod inside { pub mod outside; }\n");
    root.file(
        "types/inside/outside.rs",
        "pub struct Actual { pub id: String }\n",
    );
    // A file at the wrong path must never be accepted as the child module.
    root.file("types/outside.rs", "pub struct Decoy { pub id: String }\n");
    root.file("client.rs", CLIENT);
    let observed = inspect_generated(root.path()).expect("inline/external nested module");
    assert!(
        observed
            .structs
            .contains_key("crate::generated::types::inside::outside::Actual")
    );
    assert!(
        !observed
            .structs
            .contains_key("crate::generated::types::inside::outside::Decoy")
    );
    assert_eq!(
        observed.structs["crate::generated::types::inside::outside::Actual"]
            .location
            .file,
        "types/inside/outside.rs"
    );
}

#[test]
fn explicit_path_module_must_not_silently_use_default_path() {
    let root = Scratch::new();
    root.file("types.rs", "#[path = \"alternate.rs\"] pub mod inside;\n");
    root.file("types/inside.rs", "pub struct Decoy;\n");
    root.file("client.rs", CLIENT);
    let error = inspect_generated(root.path()).expect_err("custom module path is unproven");
    assert!(
        error
            .to_string()
            .contains("extract.module_path_unsupported")
    );
    assert!(error.to_string().contains("types.rs:1"));
}

#[test]
fn unknown_layout_or_unproved_wire_names_fail_closed_with_location() {
    let root = Scratch::new();
    root.file("client.rs", CLIENT);
    let error = inspect_generated(root.path()).expect_err("missing types.rs");
    assert!(
        error
            .to_string()
            .contains("extract.required_source_missing")
    );
    root.file("types.rs", "pub mod missing;\n");
    let error = inspect_generated(root.path()).expect_err("missing external module");
    assert!(
        error
            .to_string()
            .contains("extract.external_module_unresolved")
    );
    root.file(
        "types.rs",
        "pub struct Item { #[serde(rename_all = \"camelCase\")] pub field: String }\n",
    );
    let error = inspect_generated(root.path()).expect_err("unsupported serde rule");
    assert!(error.to_string().contains("extract.serde_unsupported"));
    assert!(error.to_string().contains("types.rs:1"));
    root.file(
        "types.rs",
        "#[serde(rename_all = \"camelCase\")] pub struct Item { pub field_name: String }\n",
    );
    let error = inspect_generated(root.path()).expect_err("container wire transform unproven");
    assert!(
        error
            .to_string()
            .contains("extract.serde_container_unproven")
    );
    root.file(
        "types.rs",
        "#[serde(untagged)] pub enum Item { Text, Count }\n",
    );
    let error =
        inspect_generated(root.path()).expect_err("untagged variant names are not wire values");
    assert!(
        error
            .to_string()
            .contains("extract.serde_container_unproven")
    );
    root.file(
        "types.rs",
        "pub struct Item { #[serde(rename(serialize = \"wire\"))] pub field: String }\n",
    );
    let error = inspect_generated(root.path()).expect_err("serialization-only rename is ambiguous");
    assert!(
        error
            .to_string()
            .contains("extract.serde_wire_name_ambiguous")
    );
    root.file(
        "types.rs",
        "pub struct Item { #[serde(skip_serializing)] pub field: String }\n",
    );
    let error =
        inspect_generated(root.path()).expect_err("directional skip requires separate projections");
    assert!(error.to_string().contains("extract.serde_unsupported"));
    root.file("types.rs", "pub struct Item { pub field: String }\n");
    root.file("client.rs", "not valid Rust");
    let error = inspect_generated(root.path()).expect_err("invalid Rust");
    assert!(error.to_string().contains("extract.rust_parse"));
    root.file("client.rs", "pub struct NotAClient;\n");
    let error = inspect_generated(root.path()).expect_err("cannot prove raw client");
    assert!(error.to_string().contains("extract.client_layout_unproven"));
}

#[test]
fn untagged_single_payload_enum_is_proven_as_payload_union() {
    let root = Scratch::new();
    root.file(
        "types.rs",
        "#[serde(untagged)] pub enum Item { Text(String), Count(u64) }\n",
    );
    root.file("client.rs", CLIENT);
    let evidence = inspect_generated(root.path()).expect("untagged payload union is structural");
    let item = &evidence.enums["crate::generated::types::Item"];
    assert_eq!(item.variants.len(), 2);
    assert_eq!(item.variants[0].payload, vec!["String"]);
    assert_eq!(item.variants[1].payload, vec!["u64"]);
    assert!(item.variants.iter().all(|variant| variant.wire_name == variant.name));
}

#[test]
fn multiple_client_candidates_and_ambiguous_aliases_are_rejected() {
    let root = Scratch::new();
    root.file("types.rs", "pub struct Model { pub field: String }");
    root.file(
        "client.rs",
        r#"
pub struct One;
pub struct Two;
impl One {
    pub fn new() -> Self { Self }
    pub async fn op(&self) -> Result<(), ()> { todo!() }
}
impl Two {
    pub fn new() -> Self { Self }
    pub async fn op(&self) -> Result<(), ()> { todo!() }
}
"#,
    );
    let error = inspect_generated(root.path()).expect_err("ambiguous client");
    assert!(error.to_string().contains("extract.client_layout_unproven"));
    root.file("client.rs", CLIENT);
    root.file("types.rs", "pub type Same = String;\npub type Same = u64;");
    let error = inspect_generated(root.path()).expect_err("duplicate unguarded alias");
    assert!(
        error
            .to_string()
            .contains("extract.duplicate_alias_without_distinct_cfg")
    );
}
