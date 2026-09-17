//! Backend-neutral Rust SDK derivation and generation.
//!
//! The root crate owns complete-definition validation, lowering and deterministic emission.
//! Concrete OpenAPI-to-Rust backends remain adapters that only produce [`Bindings`].

mod compiler;
mod contracts;
mod derivation;
mod emit;
mod error;
#[allow(dead_code)]
mod ir;
#[allow(
    clippy::collapsible_if,
    clippy::needless_lifetimes,
    clippy::unnecessary_unwrap,
    clippy::useless_format
)]
mod lower;
#[allow(dead_code)]
mod openapi;
mod reconcile;
mod rust_type;
#[allow(dead_code)]
mod symbols;
#[allow(clippy::collapsible_if)]
mod validation;

pub use contracts::{
    AccessorDefinition, AccessorKindDefinition, ApiInventory, BindingLayout, Bindings,
    ClientBinding, ClientDefinition, FieldBinding, GenerateInput, GeneratedSdk, MapDefinition,
    ModelDefinition, OpenApi, OperationBinding, OperationDefinition, ParameterBinding,
    ResourceDefinition, ResourceInventory, Runtime, ScalarEnumDefinition, SdkDefinition,
    SimpleUnionDefinition, SimpleUnionVariant, StreamBinding, StreamDefinition, UnionDefinition,
    UnionFactoryDefinition, VariantBinding,
};
pub use derivation::{
    Derivation, DerivationError, DerivationReason, DerivationReport, DerivationStatus, DeriveInput,
    OperationDerivation, PublicSdkSurface, SdkOverrides, derive,
};
pub use error::{Diagnostic, GenerationError};
pub use rust_type::{Type, TypeKind, parse_type};

/// Validate and generate an SDK from an explicit complete definition.
pub fn generate(input: GenerateInput) -> Result<GeneratedSdk, GenerationError> {
    compiler::generate(input)
}
