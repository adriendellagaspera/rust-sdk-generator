//! Normalize `openapi-to-rust` output into the versioned Rust Bindings contract.

mod details;
mod extract;
mod manifest;
mod model;
mod reader;
mod rust_type;
mod semantic;
mod structural;

pub use extract::extract_bindings;
pub use manifest::{MANIFEST_NAME, parse_binding_manifest};
pub use model::{Bindings, Error};
pub use reader::{SIDECAR_NAME, read_bindings};
pub use semantic::{
    OperationSemanticEvidence, RepresentationEvidence, SemanticEvidence, SourceOperationEvidence,
    inspect_semantics,
};
pub use structural::{EvidenceLocation, StructuralEvidence, inspect_generated};
