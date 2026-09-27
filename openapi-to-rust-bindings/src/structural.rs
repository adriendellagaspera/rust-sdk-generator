//! Metadata-backed view used only by the residual compatibility proofs.
//!
//! These values are populated from upstream bindings.json; this module does not
//! parse generated Rust.

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceLocation {
    pub file: String,
    pub line: usize,
    pub module: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FieldEvidence {
    pub name: String,
    pub rust_type: String,
    pub wire_name: Option<String>,
    pub serde_skip: bool,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StructEvidence {
    pub path: String,
    pub fields: Vec<FieldEvidence>,
    pub has_private_fields: bool,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VariantEvidence {
    pub name: String,
    pub wire_name: String,
    pub payload: Vec<String>,
    pub named_payload: Vec<FieldEvidence>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EnumEvidence {
    pub path: String,
    pub variants: Vec<VariantEvidence>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AliasEvidence {
    pub path: String,
    pub rust_type: String,
    pub cfg: Vec<String>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ParameterEvidence {
    pub name: String,
    pub rust_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MethodEvidence {
    pub name: String,
    pub parameters: Vec<ParameterEvidence>,
    pub return_type: String,
    pub success_type: Option<String>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ClientEvidence {
    pub path: String,
    pub constructors: Vec<String>,
    pub builders: Vec<String>,
    pub methods: Vec<MethodEvidence>,
    pub imports: Vec<String>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StructuralEvidence {
    pub structs: BTreeMap<String, StructEvidence>,
    pub enums: BTreeMap<String, EnumEvidence>,
    pub aliases: BTreeMap<String, Vec<AliasEvidence>>,
    pub client: ClientEvidence,
    pub semantics: &'static str,
}
