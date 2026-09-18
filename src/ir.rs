use crate::{AccessorKindDefinition, RequestMediaDefinition};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FacadeIr {
    pub client_name: String,
    pub models: Vec<ModelSpec>,
    pub resources: Vec<ResourceSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelSpec {
    pub name: String,
    pub raw: String,
    pub render: ModelRenderSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelRenderSpec {
    Wrapper(WrapperModelSpec),
    Union(UnionModelSpec),
    SimpleUnion(SimpleUnionModelSpec),
    View(ViewModelSpec),
    Alias(AliasModelSpec),
    Map(MapModelSpec),
    ScalarEnum(ScalarEnumModelSpec),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WrapperModelSpec {
    pub constructor: Option<ConstructorSpec>,
    pub factories: Vec<FactorySpec>,
    pub setters: Vec<SetterSpec>,
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConstructorSpec {
    pub arguments: Vec<ArgumentSpec>,
    pub value: StructValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FactorySpec {
    pub name: String,
    pub arguments: Vec<ArgumentSpec>,
    pub value: StructValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetterSpec {
    pub name: String,
    pub raw_field: String,
    pub argument: ArgumentSpec,
    pub value: ValueSpec,
    pub null_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArgumentKind {
    Exact,
    IntoString,
    IntoModel,
    IntoIterModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArgumentSpec {
    pub name: String,
    pub kind: ArgumentKind,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ValueSpec {
    Variable(String),
    IntoString(String),
    IntoModel {
        name: String,
        adapter: String,
    },
    CollectInto(String),
    MapInto {
        name: String,
        depth: usize,
    },
    Some {
        value: Box<ValueSpec>,
        depth: usize,
    },
    Enum {
        type_name: String,
        variant: String,
        value: Box<ValueSpec>,
    },
    Struct(StructValue),
    Literal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructFieldValue {
    pub name: String,
    pub value: ValueSpec,
    pub shorthand: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructValue {
    pub type_name: String,
    pub fields: Vec<StructFieldValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnionModelSpec {
    pub branches: Vec<UnionBranchSpec>,
    pub targets: Vec<UnionTargetSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnionBranchSpec {
    pub public_name: String,
    pub constructor_name: String,
    pub public_type: String,
    pub argument: ArgumentSpec,
    pub raw_payload: String,
    pub raw_value: ValueSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnionTargetSpec {
    pub raw: String,
    pub variants: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SimpleUnionModelSpec {
    pub branches: Vec<SimpleUnionBranchSpec>,
    pub bidirectional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SimpleUnionBranchSpec {
    pub raw_name: String,
    pub public_name: String,
    pub public_type: String,
    pub adapt_depth: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewModelSpec {
    pub borrowed: bool,
    pub accessors: Vec<ResolvedAccessor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedAccessor {
    pub name: String,
    pub kind: AccessorKindDefinition,
    pub path: Vec<String>,
    pub return_type: String,
    pub wrapper: Option<String>,
    pub enum_type: Option<String>,
    pub enum_variant: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AliasModelSpec {
    pub public_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MapModelSpec {
    pub public_type: String,
    pub raw_field: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScalarEnumModelSpec {
    pub variants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceSpec {
    pub path: Vec<String>,
    pub module: String,
    pub name: String,
    pub operations: Vec<OperationSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationSpec {
    pub name: String,
    pub operation_id: String,
    pub raw_method: String,
    pub raw_signature: RawSignature,
    pub request_projection: RequestProjection,
    pub response_projection: ResponseProjection,
    pub call: OperationCall,
    pub parameter_request: Option<ParameterRequestSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawSignature {
    pub parameters: Vec<RawParameter>,
    pub return_type: String,
    pub success_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawParameter {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequestProjection {
    None,
    Parameters,
    Model {
        media: RequestMediaDefinition,
        model: String,
        raw: String,
        overrides: Vec<(String, Option<bool>)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResponseProjection {
    Json { model: String, raw: String },
    Empty,
    Binary,
    Sse(StreamPolicy),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamPolicy {
    pub item: String,
    pub wrapper: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationCall {
    pub arguments: String,
    pub raw_arguments: String,
    pub default_raw_arguments: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParameterRequestSpec {
    pub name: String,
    pub fields: Vec<ParameterField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParameterField {
    pub name: String,
    pub type_name: String,
    pub constructor_argument: Option<String>,
    pub constructor_value: Option<String>,
    pub setter_argument: Option<String>,
    pub setter_value: Option<String>,
}
