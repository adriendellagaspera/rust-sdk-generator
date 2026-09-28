//! Thin compatibility shim from openapi-to-rust bindings metadata to the
//! backend-neutral Bindings contract consumed by rust-sdk-generator.

mod details;
mod extract;
mod rust_type;
mod semantic;
mod structural;
mod upstream_metadata;

use std::fmt;

pub use extract::extract_bindings;
pub use rust_sdk_generator::Bindings;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    message: String,
}

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Normalize the current openapi-to-rust output using its bindings metadata,
/// the exact effective OpenAPI document, and only the residual generated-code
/// evidence not yet exposed by upstream metadata.
pub fn read_bindings(
    generated: impl AsRef<std::path::Path>,
    effective_openapi: impl AsRef<std::path::Path>,
) -> Result<Bindings, Error> {
    extract_bindings(generated, effective_openapi)
}
