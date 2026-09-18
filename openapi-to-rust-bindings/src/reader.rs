use crate::{Bindings, Error, MANIFEST_NAME, parse_binding_manifest};
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Canonical sidecar name accepted when generator-owned metadata is materialized separately.
pub const SIDECAR_NAME: &str = "rust-bindings.json";

/// Read normalized bindings from generator-owned metadata or a canonical sidecar.
pub fn read_bindings(path: impl AsRef<Path>) -> Result<Bindings, Error> {
    let path = path.as_ref();

    let manifest = path.join(MANIFEST_NAME);
    if manifest.is_file() {
        let source = fs::read_to_string(&manifest).map_err(|error| {
            Error::new(format!("failed to read {}: {error}", manifest.display()))
        })?;
        return parse_binding_manifest(&source);
    }

    let sidecar = path.join(SIDECAR_NAME);
    if sidecar.is_file() {
        let value = fs::read_to_string(&sidecar)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| Bindings::from_value(value).ok())
            .ok_or_else(|| Error::new(format!("invalid {SIDECAR_NAME}")))?;
        return Ok(value);
    }

    Err(Error::new(format!(
        "missing {MANIFEST_NAME} or {SIDECAR_NAME} in {}",
        path.display()
    )))
}
