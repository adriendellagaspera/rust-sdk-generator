//! Normalize `openapi-to-rust` output into the versioned Rust Bindings contract.

mod model;
mod parser;
mod reader;

pub use model::{Bindings, Error};
pub use parser::parse_bindings;
pub use reader::{SIDECAR_NAME, read_bindings};
