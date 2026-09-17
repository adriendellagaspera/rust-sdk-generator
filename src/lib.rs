//! Backend-neutral Rust SDK derivation and generation.
//!
//! The crate is the canonical language/runtime boundary for the root generator.
//! Concrete OpenAPI-to-Rust backends remain adapters that only produce [`Bindings`].
//! Validation and lowering close over those contracts before deterministic emission.

// The closed compiler is introduced one slice before the canonical public
// generate/derive entry points make it reachable from normal library builds.
#[allow(dead_code)]
mod compiler;
mod contracts;
#[allow(dead_code)]
mod emit;
mod error;
#[allow(dead_code)]
mod ir;
#[allow(
    dead_code,
    clippy::collapsible_if,
    clippy::needless_lifetimes,
    clippy::unnecessary_unwrap,
    clippy::useless_format
)]
mod lower;
#[allow(dead_code)]
mod openapi;
mod rust_type;
#[allow(dead_code)]
mod symbols;
#[allow(dead_code, clippy::collapsible_if)]
mod validation;

pub use contracts::{
    AccessorDefinition, AccessorKindDefinition, ApiInventory, BindingLayout, Bindings,
    ClientBinding, ClientDefinition, FieldBinding, GeneratedSdk, MapDefinition, ModelDefinition,
    OpenApi, OperationBinding, OperationDefinition, ParameterBinding, ResourceDefinition,
    ResourceInventory, Runtime, ScalarEnumDefinition, SdkDefinition, SimpleUnionDefinition,
    SimpleUnionVariant, StreamBinding, StreamDefinition, UnionDefinition, UnionFactoryDefinition,
    VariantBinding,
};
pub use error::{Diagnostic, GenerationError};
pub use rust_type::{Type, TypeKind, parse_type};
