//! Backend-neutral Rust SDK derivation and generation.
//!
//! The crate is the canonical language/runtime boundary for the root generator.
//! Concrete OpenAPI-to-Rust backends remain adapters that only produce [`Bindings`].
//! Validation and lowering close over those contracts before deterministic emission.

mod compiler;
mod contracts;
mod emit;
mod error;
mod ir;
mod lower;
mod openapi;
mod rust_type;
mod symbols;
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
