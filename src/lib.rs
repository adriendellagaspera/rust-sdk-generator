//! Backend-neutral Rust SDK derivation and generation.
//!
//! The crate is the canonical language/runtime boundary for the root generator.
//! Concrete OpenAPI-to-Rust backends remain adapters that only produce [`Bindings`].

mod contracts;

pub use contracts::{
    AccessorDefinition, ApiInventory, BindingLayout, Bindings, ClientBinding, ClientDefinition,
    GeneratedSdk, MapDefinition, ModelDefinition, OpenApi, OperationBinding, OperationDefinition,
    ParameterBinding, ResourceDefinition, Runtime, ScalarEnumDefinition, SdkDefinition,
    SimpleUnionDefinition, SimpleUnionVariant, StreamBinding, StreamDefinition, UnionDefinition,
    UnionFactoryDefinition, VariantBinding,
};
