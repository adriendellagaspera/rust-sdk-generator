use openapi_to_rust_bindings::{RepresentationEvidence, extract_bindings, inspect_semantics};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bindings-semantic-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create scratch");
        Self(path)
    }
    fn file(&self, name: &str, content: &str) {
        fs::write(self.0.join(name), content).expect("write fixture");
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

const TYPES: &str = r#"
pub struct Inventory {
    pub id: String,
}
"#;

const CLIENT: &str = r#"
use super::types::*;
pub struct HttpClient {
    base_url: String,
    api_key: Option<String>,
    http_client: reqwest::Client,
}
impl HttpClient {
    pub fn new() -> Self {
        Self {
            base_url: String::new(),
            api_key: None,
            http_client: todo!(),
        }
    }
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// GET /inventory/{id}
    pub async fn fetch_inventory_without_naming_shortcut(
        &self,
        id: impl AsRef<str>,
    ) -> Result<Inventory, Error> {
        let request_url = format!("{}{}", self.base_url, format!("/inventory/{}", id.as_ref()));
        let req = self.http_client.get(request_url);
        let response = req.send().await?;
        let status = response.status();
        let body_text = response.text().await?;
        if status.is_success() {
            Ok(serde_json::from_str(&body_text)?)
        } else {
            todo!()
        }
    }

    /// POST /render
    pub async fn render_inventory(&self) -> Result<Inventory, Error> {
        let request_url = format!("{}{}", self.base_url, "/render");
        let req = self.http_client.post(request_url);
        let response = req.send().await?;
        let status = response.status();
        let body_text = response.text().await?;
        if status.is_success() {
            Ok(serde_json::from_str(&body_text)?)
        } else {
            todo!()
        }
    }

    /// DELETE /inventory/{id}
    pub async fn delete_inventory(&self, id: impl AsRef<str>) -> Result<(), Error> {
        let request_url = format!("{}{}", self.base_url, format!("/inventory/{}", id.as_ref()));
        let req = self.http_client.delete(request_url);
        let response = req.send().await?;
        let status = response.status();
        if status.is_success() { Ok(()) } else { todo!() }
    }
}
"#;

const OPENAPI: &str = r##"{
  "openapi": "3.1.0",
  "info": {"title": "semantic", "version": "1"},
  "paths": {
    "/inventory/{id}": {
      "get": {
        "operationId": "fetchInventoryWithoutNamingShortcut",
        "responses": {
          "200": {
            "content": {
              "application/json": {
                "schema": {"$ref": "#/components/schemas/Inventory"}
              }
            }
          }
        }
      },
      "delete": {
        "operationId": "deleteInventory",
        "responses": {"204": {"description": "deleted"}}
      }
    },
    "/render": {
      "post": {
        "operationId": "renderInventory",
        "responses": {
          "200": {
            "content": {
              "application/json": {
                "schema": {"$ref": "#/components/schemas/Inventory"}
              },
              "text/event-stream": {"schema": {"type": "string"}}
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "Inventory": {
        "type": "object",
        "required": ["id"],
        "properties": {"id": {"type": "string"}}
      }
    }
  }
}"##;

fn fixture(client: &str) -> Scratch {
    let root = Scratch::new();
    root.file("types.rs", TYPES);
    root.file("client.rs", client);
    root.file("openapi.json", OPENAPI);
    root
}

#[test]
fn exact_route_and_body_evidence_select_the_emitted_representation() {
    let root = fixture(CLIENT);
    let evidence = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect("semantic evidence");

    let fetch = &evidence.operations["fetch_inventory_without_naming_shortcut"];
    assert_eq!(
        fetch.source_operation.operation_id,
        "fetchInventoryWithoutNamingShortcut"
    );
    assert_eq!(fetch.source_operation.method, "GET");
    assert_eq!(fetch.source_operation.path, "/inventory/{id}");
    assert_eq!(
        fetch.emitted_operation_id,
        "fetchInventoryWithoutNamingShortcut"
    );
    assert_eq!(fetch.success_statuses, Vec::<String>::new());
    assert!(matches!(
        fetch.representation,
        RepresentationEvidence::Json {
            ref schema_name,
            ref media_type,
        } if schema_name == "Inventory" && media_type == "application/json"
    ));

    let render = &evidence.operations["render_inventory"];
    assert!(matches!(
        render.representation,
        RepresentationEvidence::Json { .. }
    ));
    let delete = &evidence.operations["delete_inventory"];
    assert_eq!(delete.representation, RepresentationEvidence::Empty);
    assert!(evidence.unsupported_stream_methods.is_empty());
}

#[test]
fn proved_non_streaming_semantics_normalize_to_bindings_v3() {
    let root = fixture(CLIENT);
    let bindings = extract_bindings(root.path(), root.path().join("openapi.json"))
        .expect("canonical bindings");
    let value = bindings.as_value();

    assert_eq!(value["schema_version"], 3);
    assert_eq!(
        value["binding"]["client"]["type_path"],
        "crate::generated::client::HttpClient"
    );
    assert_eq!(
        value["operations"]["fetch_inventory_without_naming_shortcut"]["metadata"]["source_operation"]
            ["operation_id"],
        "fetchInventoryWithoutNamingShortcut"
    );
    assert_eq!(
        value["operations"]["render_inventory"]["metadata"]["representation"]["kind"],
        "json"
    );
    assert_eq!(
        value["operations"]["delete_inventory"]["metadata"]["representation"]["kind"],
        "empty"
    );
    assert_eq!(
        value["operations"]["render_inventory"]["metadata"]["success_statuses"],
        serde_json::json!([])
    );
    assert_eq!(
        value["symbol_paths"]["Inventory"],
        "crate::generated::types::Inventory"
    );
}

#[test]
fn route_doc_is_not_accepted_without_matching_http_body_evidence() {
    let client = CLIENT.replacen(
        "let req = self.http_client.get(request_url);",
        "let req = self.http_client.post(request_url);",
        1,
    );
    let root = fixture(&client);
    let error = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect_err("verb mismatch must fail");
    assert!(
        error
            .to_string()
            .contains("extract.source_identity_ambiguous")
    );
}

#[test]
fn analyzer_or_base_method_rename_fails_when_emitted_id_is_not_provable() {
    let client = CLIENT.replacen(
        "fetch_inventory_without_naming_shortcut(",
        "fetch_inventory_without_naming_shortcut_2(",
        1,
    );
    let root = fixture(&client);
    let error = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect_err("source identity alone does not prove the emitted analyzer id");
    assert!(error.to_string().contains("extract.emitted_id_unproven"));
}

#[test]
fn duplicate_source_operation_ids_recover_the_backend_emitted_id() {
    let client = CLIENT.replacen(
        "render_inventory(&self)",
        "fetch_inventory_without_naming_shortcut_post(&self)",
        1,
    );
    let root = fixture(&client);
    let mut openapi: serde_json::Value = serde_json::from_str(OPENAPI).expect("fixture JSON");
    openapi["paths"]["/render"]["post"]["operationId"] =
        serde_json::Value::String("fetchInventoryWithoutNamingShortcut".into());
    root.file(
        "openapi.json",
        &serde_json::to_string_pretty(&openapi).expect("serialize OpenAPI"),
    );
    let evidence = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect("backend duplicate-ID allocation is observable from the emitted base method");
    let duplicate = &evidence.operations["fetch_inventory_without_naming_shortcut_post"];
    assert_eq!(
        duplicate.source_operation.operation_id,
        "fetchInventoryWithoutNamingShortcut"
    );
    assert_eq!(
        duplicate.emitted_operation_id,
        "fetchInventoryWithoutNamingShortcut_post"
    );
    assert_eq!(
        duplicate.rust_method_name,
        "fetch_inventory_without_naming_shortcut_post"
    );
}

#[test]
fn source_operations_without_emitted_methods_fail_closed() {
    let root = fixture(CLIENT);
    let mut openapi: serde_json::Value = serde_json::from_str(OPENAPI).expect("fixture JSON");
    openapi["paths"]["/ghost"] = serde_json::json!({
        "get": {
            "operationId": "ghostOperation",
            "responses": {
                "204": {"description": "never emitted"}
            }
        }
    });
    root.file(
        "openapi.json",
        &serde_json::to_string_pretty(&openapi).expect("serialize OpenAPI"),
    );
    let error = extract_bindings(root.path(), root.path().join("openapi.json"))
        .expect_err("unemitted source operation must be rejected");
    assert!(
        error
            .to_string()
            .contains("extract.source_operation_unemitted")
    );
}

#[test]
fn anonymous_stream_return_type_fails_without_a_proven_cross_target_abi() {
    const STREAM_CLIENT: &str = r#"
use super::types::*;
pub struct HttpClient {
    base_url: String,
    api_key: Option<String>,
    http_client: reqwest::Client,
}
impl HttpClient {
    pub fn new() -> Self {
        Self {
            base_url: String::new(),
            api_key: None,
            http_client: todo!(),
        }
    }
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// GET /events
    pub async fn events(
        &self,
    ) -> Result<
        impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>>,
        Error,
    > {
        let request_url = format!("{}{}", self.base_url, "/events");
        let mut req = self.http_client.get(request_url);
        req = req.header(reqwest::header::ACCEPT, "text/event-stream");
        let response = req.send().await?;
        let status = response.status();
        let status_code = status.as_u16();
        if status_code == 200 {
            Ok(response.bytes_stream())
        } else {
            todo!()
        }
    }
}
"#;
    const STREAM_OPENAPI: &str = r#"{
      "openapi": "3.1.0",
      "info": {"title": "stream", "version": "1"},
      "paths": {
        "/events": {
          "get": {
            "operationId": "events",
            "responses": {
              "200": {
                "description": "events",
                "content": {
                  "text/event-stream": {"schema": {"type": "string"}}
                }
              }
            }
          }
        }
      }
    }"#;

    let root = Scratch::new();
    root.file("types.rs", "");
    root.file("client.rs", STREAM_CLIENT);
    root.file("openapi.json", STREAM_OPENAPI);
    let error = extract_bindings(root.path(), root.path().join("openapi.json"))
        .expect_err("anonymous streams have no proven native/WASM alias ABI");
    assert!(error.to_string().contains("extract.stream_abi_unproven"));
}


fn assert_extract_error(root: &Scratch, code: &str, context: &str) {
    let error = extract_bindings(root.path(), root.path().join("openapi.json"))
        .expect_err("fixture must fail closed");
    let message = error.to_string();
    assert!(message.contains(code), "{message}");
    assert!(message.contains(context), "{message}");
}

#[test]
fn client_constructor_signature_must_match_the_facade_contract() {
    let client = CLIENT.replacen(
        "pub fn new() -> Self",
        "pub fn new(seed: String) -> Self",
        1,
    );
    let root = fixture(&client);
    assert_extract_error(&root, "extract.client_layout_unproven", "::new");
}

#[test]
fn base_url_builder_must_mutate_the_returned_base_url_state() {
    let client = CLIENT.replacen(
        "self.base_url = base_url.into();",
        "self.api_key = Some(base_url.into());",
        1,
    );
    let root = fixture(&client);
    assert_extract_error(
        &root,
        "extract.client_layout_unproven",
        "with_base_url",
    );
}

#[test]
fn api_key_builder_must_mutate_the_returned_api_key_state() {
    let client = CLIENT.replacen(
        "self.api_key = Some(api_key.into());",
        "self.base_url = api_key.into();",
        1,
    );
    let root = fixture(&client);
    assert_extract_error(&root, "extract.client_layout_unproven", "with_api_key");
}

#[test]
fn route_skeleton_collisions_are_rejected_as_ambiguous_source_identity() {
    let root = fixture(CLIENT);
    let mut openapi: serde_json::Value = serde_json::from_str(OPENAPI).expect("fixture JSON");
    openapi["paths"]["/inventory/{name}"] = serde_json::json!({
        "get": {
            "operationId": "fetchInventoryByName",
            "responses": {
                "200": {
                    "content": {
                        "application/json": {
                            "schema": {"$ref": "#/components/schemas/Inventory"}
                        }
                    }
                }
            }
        }
    });
    root.file(
        "openapi.json",
        &serde_json::to_string_pretty(&openapi).expect("serialize OpenAPI"),
    );
    let error = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect_err("same verb and route skeleton must be ambiguous");
    let message = error.to_string();
    assert!(message.contains("extract.source_identity_ambiguous"), "{message}");
    assert!(
        message.contains("fetch_inventory_without_naming_shortcut"),
        "{message}"
    );
}

#[test]
fn duplicate_operation_id_allocation_must_be_observable_from_the_emitted_method() {
    let client = CLIENT.replacen(
        "render_inventory(&self)",
        "fetch_inventory_without_naming_shortcut_2(&self)",
        1,
    );
    let root = fixture(&client);
    let mut openapi: serde_json::Value = serde_json::from_str(OPENAPI).expect("fixture JSON");
    openapi["paths"]["/render"]["post"]["operationId"] =
        serde_json::Value::String("fetchInventoryWithoutNamingShortcut".into());
    root.file(
        "openapi.json",
        &serde_json::to_string_pretty(&openapi).expect("serialize OpenAPI"),
    );
    let error = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect_err("unsupported collision allocation must fail closed");
    let message = error.to_string();
    assert!(message.contains("extract.emitted_id_unproven"), "{message}");
    assert!(
        message.contains("fetchInventoryWithoutNamingShortcut"),
        "{message}"
    );
}

#[test]
fn multiple_top_level_status_guards_are_rejected() {
    let client = CLIENT.replacen(
        "let status = response.status();",
        "let status = response.status();\n        if status.is_success() {}",
        1,
    );
    let root = fixture(&client);
    let error = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect_err("ambiguous success guards must fail closed");
    let message = error.to_string();
    assert!(
        message.contains("extract.success_statuses_unproven"),
        "{message}"
    );
    assert!(
        message.contains("fetch_inventory_without_naming_shortcut"),
        "{message}"
    );
}

#[test]
fn emitted_success_status_must_be_declared_by_the_source_operation() {
    let client = CLIENT
        .replacen(
            "let status = response.status();",
            "let status = response.status();\n        let status_code = status.as_u16();",
            1,
        )
        .replacen("if status.is_success()", "if status_code == 201", 1);
    let root = fixture(&client);
    let error = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect_err("generated 201 guard cannot be attributed to an OpenAPI 200 response");
    let message = error.to_string();
    assert!(
        message.contains("extract.success_statuses_unproven"),
        "{message}"
    );
    assert!(message.contains("201"), "{message}");
    assert!(
        message.contains("fetch_inventory_without_naming_shortcut"),
        "{message}"
    );
}

#[test]
fn response_representation_is_selected_only_from_emitted_statuses() {
    let client = CLIENT
        .replacen(
            "let status = response.status();",
            "let status = response.status();\n        let status_code = status.as_u16();",
            1,
        )
        .replacen("if status.is_success()", "if status_code == 200", 1);
    let root = fixture(&client);
    let mut openapi: serde_json::Value = serde_json::from_str(OPENAPI).expect("fixture JSON");
    openapi["paths"]["/inventory/{id}"]["get"]["responses"]["206"] = serde_json::json!({
        "content": {
            "application/octet-stream": {
                "schema": {"type": "string", "format": "binary"}
            }
        }
    });
    root.file(
        "openapi.json",
        &serde_json::to_string_pretty(&openapi).expect("serialize OpenAPI"),
    );
    let evidence = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect("the unselected 206 representation must not make 200 ambiguous");
    let fetch = &evidence.operations["fetch_inventory_without_naming_shortcut"];
    assert_eq!(fetch.success_statuses, vec!["200"]);
    assert!(matches!(
        fetch.representation,
        RepresentationEvidence::Json { .. }
    ));
}

#[test]
fn empty_success_type_is_rejected_when_selected_source_response_has_content() {
    let root = fixture(CLIENT);
    let mut openapi: serde_json::Value = serde_json::from_str(OPENAPI).expect("fixture JSON");
    openapi["paths"]["/inventory/{id}"]["delete"]["responses"]["204"]["content"] =
        serde_json::json!({
            "application/json": {
                "schema": {"$ref": "#/components/schemas/Inventory"}
            }
        });
    root.file(
        "openapi.json",
        &serde_json::to_string_pretty(&openapi).expect("serialize OpenAPI"),
    );
    let error = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect_err("bodyless generated success cannot represent declared response content");
    let message = error.to_string();
    assert!(message.contains("extract.representation_unproven"), "{message}");
    assert!(message.contains("delete_inventory"), "{message}");
}


const MULTIPART_TYPES: &str = r#"
pub struct UploadRequest {
    pub file: Vec<u8>,
}
"#;

const MULTIPART_CLIENT: &str = r#"
use super::types::*;
pub struct HttpClient {
    base_url: String,
    api_key: Option<String>,
    http_client: reqwest::Client,
}
impl HttpClient {
    pub fn new() -> Self {
        Self {
            base_url: String::new(),
            api_key: None,
            http_client: todo!(),
        }
    }
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// POST /uploads
    pub async fn upload(&self, request: UploadRequest) -> Result<(), Error> {
        let request_url = format!("{}{}", self.base_url, "/uploads");
        let mut form = reqwest::multipart::Form::new();
        form = form.part("file", reqwest::multipart::Part::bytes(request.file));
        let req = self.http_client.post(request_url).multipart(form);
        let response = req.send().await?;
        let status_code = response.status().as_u16();
        if status_code == 204 { Ok(()) } else { todo!() }
    }

    /// POST /uploads
    pub async fn upload_with_multipart_filenames(
        &self,
        request: UploadRequest,
        multipart_filenames: &[(&str, &str)],
    ) -> Result<(), Error> {
        let request_url = format!("{}{}", self.base_url, "/uploads");
        let value = &request.file;
        let part = reqwest::multipart::Part::bytes(value.to_vec());
        let part = if let Some((_, filename)) = multipart_filenames
            .iter()
            .find(|(field, _)| *field == "file")
        {
            part.file_name((*filename).to_string())
        } else {
            part
        };
        let mut form = reqwest::multipart::Form::new();
        form = form.part("file", part);
        let req = self.http_client.post(request_url).multipart(form);
        let response = req.send().await?;
        let status_code = response.status().as_u16();
        if status_code == 204 { Ok(()) } else { todo!() }
    }
}
"#;

const MULTIPART_OPENAPI: &str = r##"{
  "openapi": "3.1.0",
  "info": {"title": "multipart", "version": "1"},
  "paths": {
    "/uploads": {
      "post": {
        "operationId": "upload",
        "requestBody": {
          "required": true,
          "content": {
            "multipart/form-data": {
              "schema": {"$ref": "#/components/schemas/UploadRequest"}
            }
          }
        },
        "responses": {"204": {"description": "uploaded"}}
      }
    }
  },
  "components": {
    "schemas": {
      "UploadRequest": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": {"type": "string", "format": "binary"}
        }
      }
    }
  }
}"##;

fn multipart_fixture(client: &str) -> Scratch {
    let root = Scratch::new();
    root.file("types.rs", MULTIPART_TYPES);
    root.file("client.rs", client);
    root.file("openapi.json", MULTIPART_OPENAPI);
    root
}

#[test]
fn multipart_filename_helper_requires_observable_filename_application() {
    let root = multipart_fixture(MULTIPART_CLIENT);
    let bindings = extract_bindings(root.path(), root.path().join("openapi.json"))
        .expect("filename lookup must be tied to emitted multipart part construction");
    assert_eq!(
        bindings.as_value()["operations"]["upload_with_multipart_filenames"]["metadata"]["kind"],
        "multipart_filenames"
    );

    let client = MULTIPART_CLIENT.replace(
        r#"        let part = if let Some((_, filename)) = multipart_filenames
            .iter()
            .find(|(field, _)| *field == "file")
        {
            part.file_name((*filename).to_string())
        } else {
            part
        };
"#,
        "",
    );
    let root = multipart_fixture(&client);
    assert_extract_error(
        &root,
        "extract.multipart_helper_unproven",
        "upload_with_multipart_filenames",
    );
}


const DISCRIMINATOR_TYPES: &str = r#"
pub struct RenderRequest {
    #[serde(rename = "live-output")]
    pub live_output: bool,
    pub mode: Option<String>,
    #[serde(rename = "nullable-mode")]
    pub nullable_mode: Option<String>,
    pub tri: Option<Option<String>>,
}
pub struct RenderResult {
    pub id: String,
}
"#;

const FIRST_DISCRIMINATOR_BLOCK: &str = r#"        {
            let __request_discriminator_value: bool =
                serde_json::from_value(serde_json::Value::Bool(true))
                    .map_err(HttpError::serialization_error)?;
            request.live_output = __request_discriminator_value;
        }
"#;

const DISCRIMINATOR_CLIENT: &str = r#"
use super::types::*;
pub struct HttpClient {
    base_url: String,
    api_key: Option<String>,
    http_client: reqwest::Client,
}
impl HttpClient {
    pub fn new() -> Self {
        Self {
            base_url: String::new(),
            api_key: None,
            http_client: todo!(),
        }
    }
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// POST /render
    pub async fn render(&self, request: RenderRequest) -> Result<RenderResult, Error> {
        let request_url = format!("{}{}", self.base_url, "/render");
        let mut req = self.http_client.post(request_url);
        let mut request = request;
        {
            let __request_discriminator_value: bool =
                serde_json::from_value(serde_json::Value::Bool(true))
                    .map_err(HttpError::serialization_error)?;
            request.live_output = __request_discriminator_value;
        }
        {
            let __request_discriminator_value: String =
                serde_json::from_value(serde_json::Value::String("fast".to_string()))
                    .map_err(HttpError::serialization_error)?;
            request.mode = Some(__request_discriminator_value);
        }
        {
            let __request_discriminator_value: String =
                serde_json::from_value(serde_json::Value::String("nullable".to_string()))
                    .map_err(HttpError::serialization_error)?;
            request.nullable_mode = Some(__request_discriminator_value);
        }
        {
            let __request_discriminator_value: String =
                serde_json::from_value(serde_json::Value::String("tri".to_string()))
                    .map_err(HttpError::serialization_error)?;
            request.tri = Some(Some(__request_discriminator_value));
        }
        req = req.body(
            serde_json::to_vec(&request).map_err(HttpError::serialization_error)?
        );
        let response = req.send().await?;
        let status_code = response.status().as_u16();
        let body_text = response.text().await?;
        if status_code == 200 {
            Ok(serde_json::from_str(&body_text)?)
        } else {
            todo!()
        }
    }
}
"#;

const DISCRIMINATOR_OPENAPI: &str = r##"{
  "openapi": "3.1.0",
  "info": {"title": "discriminators", "version": "1"},
  "paths": {
    "/render": {
      "post": {
        "operationId": "render",
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {"$ref": "#/components/schemas/RenderRequest"}
            }
          }
        },
        "responses": {
          "200": {
            "content": {
              "application/json": {
                "schema": {"$ref": "#/components/schemas/RenderResult"}
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "RenderRequest": {
        "type": "object",
        "required": ["live-output", "nullable-mode"],
        "properties": {
          "live-output": {"type": "boolean"},
          "mode": {"type": "string"},
          "nullable-mode": {"type": ["string", "null"]},
          "tri": {"type": ["string", "null"]}
        }
      },
      "RenderResult": {
        "type": "object",
        "required": ["id"],
        "properties": {"id": {"type": "string"}}
      }
    }
  }
}"##;

fn discriminator_fixture(client: &str) -> Scratch {
    let root = Scratch::new();
    root.file("types.rs", DISCRIMINATOR_TYPES);
    root.file("client.rs", client);
    root.file("openapi.json", DISCRIMINATOR_OPENAPI);
    root
}

#[test]
fn discriminator_evidence_proves_scope_value_type_and_field_semantics() {
    let root = discriminator_fixture(DISCRIMINATOR_CLIENT);
    let bindings = extract_bindings(root.path(), root.path().join("openapi.json"))
        .expect("direct pre-serialization discriminator assignments are observable");
    let discriminators = bindings.as_value()["operations"]["render"]["metadata"]
        ["request_discriminators"]
        .as_array()
        .expect("discriminator metadata");
    assert_eq!(discriminators.len(), 4);

    assert_eq!(discriminators[0]["wire_name"], "live-output");
    assert_eq!(discriminators[0]["rust_value_type"], "bool");
    assert_eq!(discriminators[0]["value"], true);
    assert_eq!(discriminators[0]["field_required"], true);
    assert_eq!(discriminators[0]["field_nullable"], false);
    assert_eq!(discriminators[0]["field_tri_state"], false);

    assert_eq!(discriminators[1]["wire_name"], "mode");
    assert_eq!(discriminators[1]["rust_value_type"], "String");
    assert_eq!(discriminators[1]["field_required"], false);
    assert_eq!(discriminators[1]["field_nullable"], false);
    assert_eq!(discriminators[1]["field_tri_state"], false);

    assert_eq!(discriminators[2]["wire_name"], "nullable-mode");
    assert_eq!(discriminators[2]["rust_value_type"], "String");
    assert_eq!(discriminators[2]["field_required"], true);
    assert_eq!(discriminators[2]["field_nullable"], true);
    assert_eq!(discriminators[2]["field_tri_state"], false);

    assert_eq!(discriminators[3]["wire_name"], "tri");
    assert_eq!(discriminators[3]["rust_value_type"], "String");
    assert_eq!(discriminators[3]["field_required"], false);
    assert_eq!(discriminators[3]["field_nullable"], true);
    assert_eq!(discriminators[3]["field_tri_state"], true);
}

#[test]
fn discriminator_evidence_rejects_unrelated_or_nested_assignments() {
    let unrelated = DISCRIMINATOR_CLIENT.replacen(
        "request.live_output = __request_discriminator_value;",
        "other.live_output = __request_discriminator_value;",
        1,
    );
    let root = discriminator_fixture(&unrelated);
    assert_extract_error(
        &root,
        "extract.request_discriminator_unproven",
        "render",
    );

    let nested = DISCRIMINATOR_CLIENT.replacen(
        "request.live_output = __request_discriminator_value;",
        "request.options.live_output = __request_discriminator_value;",
        1,
    );
    let root = discriminator_fixture(&nested);
    assert_extract_error(
        &root,
        "extract.request_discriminator_unproven",
        "nested discriminator path",
    );
}

#[test]
fn discriminator_evidence_rejects_conditional_or_post_serialization_mutation() {
    let conditional_block = FIRST_DISCRIMINATOR_BLOCK
        .replace("        {\n", "        if true {\n", 1);
    let conditional = DISCRIMINATOR_CLIENT.replacen(
        FIRST_DISCRIMINATOR_BLOCK,
        &conditional_block,
        1,
    );
    let root = discriminator_fixture(&conditional);
    assert_extract_error(
        &root,
        "extract.request_discriminator_unproven",
        "outside a direct generated assignment block",
    );

    let moved = DISCRIMINATOR_CLIENT
        .replacen(FIRST_DISCRIMINATOR_BLOCK, "", 1)
        .replacen(
            r#"        req = req.body(
            serde_json::to_vec(&request).map_err(HttpError::serialization_error)?
        );
"#,
            &format!(
                r#"        req = req.body(
            serde_json::to_vec(&request).map_err(HttpError::serialization_error)?
        );
{FIRST_DISCRIMINATOR_BLOCK}"#
            ),
            1,
        );
    let root = discriminator_fixture(&moved);
    assert_extract_error(
        &root,
        "extract.request_discriminator_unproven",
        "before serialization",
    );
}


const STREAM_ALIAS_CLIENT: &str = r#"
use super::types::*;
#[cfg(not(target_arch = "wasm32"))]
pub type HttpResponseByteStream = __NATIVE__;
#[cfg(target_arch = "wasm32")]
pub type HttpResponseByteStream = __WASM__;

pub struct HttpClient {
    base_url: String,
    api_key: Option<String>,
    http_client: reqwest::Client,
}
impl HttpClient {
    pub fn new() -> Self {
        Self {
            base_url: String::new(),
            api_key: None,
            http_client: todo!(),
        }
    }
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// GET /events
    pub async fn events(&self) -> Result<HttpResponseByteStream, Error> {
        let request_url = format!("{}{}", self.base_url, "/events");
        let mut req = self.http_client.get(request_url);
        req = req.header(reqwest::header::ACCEPT, "text/event-stream");
        let response = req.send().await?;
        let status_code = response.status().as_u16();
        if status_code == 200 {
            Ok(response.bytes_stream())
        } else {
            todo!()
        }
    }
}
"#;

const STREAM_ALIAS_OPENAPI: &str = r#"{
  "openapi": "3.1.0",
  "info": {"title": "stream aliases", "version": "1"},
  "paths": {
    "/events": {
      "get": {
        "operationId": "events",
        "responses": {
          "200": {
            "description": "events",
            "content": {
              "text/event-stream": {"schema": {"type": "string"}}
            }
          }
        }
      }
    }
  }
}"#;

const NATIVE_STREAM: &str =
    "futures_util::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>";
const WASM_STREAM: &str =
    "futures_util::stream::LocalBoxStream<'static, Result<bytes::Bytes, reqwest::Error>>";

fn stream_alias_fixture(native: &str, wasm: Option<&str>) -> Scratch {
    let mut client = STREAM_ALIAS_CLIENT
        .replace("__NATIVE__", native)
        .replace("__WASM__", wasm.unwrap_or(WASM_STREAM));
    if wasm.is_none() {
        let wasm_definition = format!(
            "#[cfg(target_arch = \"wasm32\")]\npub type HttpResponseByteStream = {WASM_STREAM};\n"
        );
        client = client.replace(&wasm_definition, "");
    }
    let root = Scratch::new();
    root.file("types.rs", "");
    root.file("client.rs", &client);
    root.file("openapi.json", STREAM_ALIAS_OPENAPI);
    root
}

#[test]
fn owned_native_and_wasm_stream_aliases_are_proved_exactly() {
    let root = stream_alias_fixture(NATIVE_STREAM, Some(WASM_STREAM));
    let bindings = extract_bindings(root.path(), root.path().join("openapi.json"))
        .expect("owned cross-target byte stream ABI");
    let stream = &bindings.as_value()["operations"]["events"]["stream"];
    assert_eq!(stream["item_type"], "bytes::Bytes");
    assert_eq!(stream["error_type"], "reqwest::Error");
    assert_eq!(stream["lifetime"], "'static");
}

#[test]
fn stream_aliases_fail_closed_for_invalid_target_item_error_and_lifetime_abi() {
    let cases = [
        (
            NATIVE_STREAM,
            "futures_util::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>",
        ),
        (
            NATIVE_STREAM,
            "futures_util::stream::LocalBoxStream<'static, Result<String, reqwest::Error>>",
        ),
        (
            "futures_util::stream::BoxStream<'static, Result<String, reqwest::Error>>",
            "futures_util::stream::LocalBoxStream<'static, Result<String, reqwest::Error>>",
        ),
        (
            "futures_util::stream::BoxStream<'static, Result<bytes::Bytes, std::io::Error>>",
            "futures_util::stream::LocalBoxStream<'static, Result<bytes::Bytes, std::io::Error>>",
        ),
        (
            "futures_util::stream::BoxStream<'a, Result<bytes::Bytes, reqwest::Error>>",
            "futures_util::stream::LocalBoxStream<'a, Result<bytes::Bytes, reqwest::Error>>",
        ),
        (
            NATIVE_STREAM,
            "futures_util::stream::LocalBoxStream<'a, Result<bytes::Bytes, reqwest::Error>>",
        ),
    ];
    for (native, wasm) in cases {
        let root = stream_alias_fixture(native, Some(wasm));
        assert_extract_error(&root, "extract.stream_abi_unproven", "events");
    }

    let root = stream_alias_fixture(NATIVE_STREAM, None);
    assert_extract_error(&root, "extract.stream_abi_unproven", "events");
}
