//! Operation semantics projected directly from upstream bindings metadata.

use crate::structural::EvidenceLocation;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct SourceOperationEvidence {
    pub operation_id: String,
    pub method: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepresentationEvidence {
    Json {
        schema_name: String,
        media_type: String,
    },
    Text {
        media_type: String,
    },
    BinaryBuffered {
        media_type: String,
        wildcard: bool,
    },
    EventStream {
        media_type: String,
    },
    BinaryStream {
        media_type: String,
        wildcard: bool,
    },
    Empty,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OperationSemanticEvidence {
    pub rust_method_name: String,
    pub source_operation: SourceOperationEvidence,
    pub emitted_operation_id: String,
    pub representation: RepresentationEvidence,
    pub success_statuses: Vec<String>,
    pub location: EvidenceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticEvidence {
    pub operations: BTreeMap<String, OperationSemanticEvidence>,
    pub unsupported_stream_methods: Vec<String>,
    pub unmatched_source_operations: Vec<SourceOperationEvidence>,
}
