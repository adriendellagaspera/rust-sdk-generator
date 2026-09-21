use openapi_to_rust_bindings::{RepresentationEvidence, inspect_semantics};
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
pub struct HttpClient {
    base_url: String,
    http_client: reqwest::Client,
}
impl HttpClient {
    pub fn new() -> Self { todo!() }

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

const OPENAPI: &str = r#"{
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
}"#;

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
    assert_eq!(fetch.source_operation.operation_id, "fetchInventoryWithoutNamingShortcut");
    assert_eq!(fetch.source_operation.method, "GET");
    assert_eq!(fetch.source_operation.path, "/inventory/{id}");
    assert_eq!(fetch.emitted_operation_id, "fetchInventoryWithoutNamingShortcut");
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
fn route_doc_is_not_accepted_without_matching_http_body_evidence() {
    let client = CLIENT.replacen(
        "let req = self.http_client.get(request_url);",
        "let req = self.http_client.post(request_url);",
        1,
    );
    let root = fixture(&client);
    let error = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect_err("verb mismatch must fail");
    assert!(error.to_string().contains("extract.source_identity_ambiguous"));
}

#[test]
fn allocated_rust_method_name_remains_distinct_from_the_source_operation_id() {
    let client = CLIENT.replacen(
        "fetch_inventory_without_naming_shortcut(",
        "fetch_inventory_without_naming_shortcut_2(",
        1,
    );
    let root = fixture(&client);
    let evidence = inspect_semantics(root.path(), root.path().join("openapi.json"))
        .expect("route/body evidence identifies the source independently of Rust naming");
    let operation = &evidence.operations["fetch_inventory_without_naming_shortcut_2"];
    assert_eq!(
        operation.source_operation.operation_id,
        "fetchInventoryWithoutNamingShortcut"
    );
    assert_eq!(
        operation.emitted_operation_id,
        "fetchInventoryWithoutNamingShortcut"
    );
    assert_eq!(
        operation.rust_method_name,
        "fetch_inventory_without_naming_shortcut_2"
    );
}
