//! Backend-neutral Rust SDK derivation and generation.
//!
//! The crate is the canonical language/runtime boundary for the root generator.
//! Concrete OpenAPI-to-Rust backends remain adapters that only produce [`Bindings`].

mod contracts;
mod error;
// These compiler-front-end primitives are introduced one review slice before lowering consumes them.
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
