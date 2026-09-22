//! Backend-neutral Rust SDK derivation and generation.
//!
//! The root crate owns closed-world SDK derivation, complete-definition validation, lowering and
//! deterministic emission. Concrete OpenAPI-to-Rust backends remain adapters that only produce
//! [`Bindings`].

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
mod naming;
#[allow(dead_code)]
mod openapi;
mod projection;
mod reconcile;
mod rust_type;
#[allow(dead_code)]
mod structural;
#[allow(dead_code)]
mod symbols;
#[allow(clippy::collapsible_if)]
mod validation;

pub use contracts::{
    AccessorDefinition, AccessorKindDefinition, ApiInventory, BindingLayout, Bindings,
    ClientBinding, ClientDefinition, FieldBinding, GenerateInput, GeneratedSdk, MapDefinition,
    ModelDefinition, OpenApi, OperationBinding, OperationBindingKind, OperationDefinition,
    OperationMetadataBinding, ParameterBinding, RequestDiscriminatorBinding,
    RequestDiscriminatorValue, RequestMediaDefinition, ResourceDefinition, ResourceInventory,
    ResponseRepresentationBinding, ResponseRepresentationDefinition, Runtime, ScalarEnumDefinition,
    SdkDefinition, SimpleUnionDefinition, SimpleUnionVariant, SourceOperationBinding,
    StreamAbiBinding, StreamBinding, StreamDefinition, UnionDefinition, UnionFactoryDefinition,
    VariantBinding,
};
pub use derivation::{
    Derivation, DerivationError, DerivationReason, DerivationReport, DerivationStatus, DeriveInput,
    OperationDerivation, OperationOverride, PublicSdkSurface, SdkOverrides, derive,
};
pub use error::{Diagnostic, GenerationError};
pub use rust_type::{Type, TypeKind, parse_type};

/// Validate and generate an SDK from a complete explicit or derived definition.
pub fn generate(input: GenerateInput) -> Result<GeneratedSdk, GenerationError> {
    compiler::generate(input)
}
