//! Normalize `openapi-to-rust` output into the versioned Rust Bindings contract.

mod manifest;
mod model;
mod reader;

pub use manifest::{MANIFEST_NAME, parse_binding_manifest};
pub use model::{Bindings, Error};
pub use reader::{SIDECAR_NAME, read_bindings};
