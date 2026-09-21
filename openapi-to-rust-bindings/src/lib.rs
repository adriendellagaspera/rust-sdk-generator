//! Normalize `openapi-to-rust` output into the versioned Rust Bindings contract.

mod manifest;
mod model;
mod reader;
mod structural;
mod semantic;

pub use manifest::{MANIFEST_NAME, parse_binding_manifest};
pub use model::{Bindings, Error};
pub use reader::{SIDECAR_NAME, read_bindings};
pub use structural::{StructuralEvidence, inspect_generated};
pub use semantic::{OperationSemanticEvidence, RepresentationEvidence, SemanticEvidence, SourceOperationEvidence, inspect_semantics};
