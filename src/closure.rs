//! Audit the lowered public SDK surface before rendering Rust source.
//!
//! Public transport interop is emitted crate-private; the strict gate therefore
//! rejects every backend-owned symbol that remains reachable from consumer signatures.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::contracts::{Bindings, GenerateInput};
use crate::error::{GenerationError, Result};
use crate::ir::{FacadeIr, ModelRenderSpec, RequestProjection, ResponseProjection};
use crate::lower;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FacadeLeakKind {
    ConsumerSignature,
    TransportInterop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacadeLeak {
    /// Public model, variant, constructor, accessor, resource method, or request field.
    pub path: String,
    pub kind: FacadeLeakKind,
    /// The signature component containing the backend symbol.
    pub type_name: String,
    pub symbol: String,
    pub symbol_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FacadeReport {
    pub leaks: Vec<FacadeLeak>,
}

impl FacadeReport {
    pub fn is_closed(&self) -> bool {
        self.leaks.is_empty()
    }

    pub fn consumer_leaks(&self) -> impl Iterator<Item = &FacadeLeak> {
        self.leaks
            .iter()
            .filter(|leak| leak.kind == FacadeLeakKind::ConsumerSignature)
    }

    /// Fail with the first deterministic, actionable leak. The full report remains
    /// available from `inspect_public_facade` for incomplete SDKs.
    pub fn require_closed(&self) -> Result<()> {
        let Some(leak) = self.leaks.first() else {
            return Ok(());
        };
        Err(GenerationError::at(
            "facade.raw_symbol_exposed",
            leak.path.clone(),
            format!(
                "public SDK {} exposes backend symbol {} ({}) through {}; project it to a public SDK type or make transport interop private",
                leak.path, leak.symbol, leak.symbol_path, leak.type_name
            ),
        ))
    }
}

/// Report backend-owned types reachable through emitted public declarations.
/// This uses exactly the IR consumed by emission, after normal lowering and validation.
pub fn inspect_public_facade(input: &GenerateInput) -> Result<FacadeReport> {
    let ir = lower::lower(
        &input.openapi,
        &input.bindings,
        &input.definition,
        &input.runtime,
    )?;
    Ok(inspect(&ir, &input.bindings))
}

/// Strict public closure gate used by generation and available independently to callers.
pub fn validate_public_facade(input: &GenerateInput) -> Result<()> {
    inspect_public_facade(input)?.require_closed()
}

// Extract Rust paths from type/signature syntax, including deeply nested generics,
// trait bounds, tuple/array types and associated types. Whitespace and punctuation
// delimit paths; quoted strings and lifetimes cannot name backend-owned types.
fn paths(syntax: &str) -> Vec<&str> {
    let bytes = syntax.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            index += 1;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index += 2;
                } else if bytes[index] == b'"' {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
            continue;
        }
        if bytes[index] == b'\'' {
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            continue;
        }
        if !bytes[index].is_ascii_alphabetic() && bytes[index] != b'_' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < bytes.len() {
            if bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_' {
                index += 1;
            } else if bytes[index..].starts_with(b"::")
                && index + 2 < bytes.len()
                && (bytes[index + 2].is_ascii_alphabetic() || bytes[index + 2] == b'_')
            {
                index += 2;
            } else {
                break;
            }
        }
        result.push(&syntax[start..index]);
    }
    result
}

pub(crate) fn inspect(ir: &FacadeIr, bindings: &Bindings) -> FacadeReport {
    let owned: BTreeMap<&str, (&str, &str)> = bindings
        .symbol_paths
        .iter()
        .flat_map(|(name, path)| {
            [
                (name.as_str(), (name.as_str(), path.as_str())),
                (path.as_str(), (name.as_str(), path.as_str())),
            ]
        })
        .collect();
    let public_models: BTreeSet<_> = ir.models.iter().map(|model| model.name.as_str()).collect();
    let mut leaks = Vec::new();
    let mut check = |path: String, kind: FacadeLeakKind, type_name: &str| {
        let mut seen = BTreeSet::new();
        for token in paths(type_name) {
            if kind == FacadeLeakKind::ConsumerSignature && public_models.contains(token) {
                continue;
            }
            if let Some(&(symbol, symbol_path)) = owned.get(token)
                && seen.insert(symbol)
            {
                leaks.push(FacadeLeak {
                    path: path.clone(),
                    kind: kind.clone(),
                    type_name: type_name.to_owned(),
                    symbol: symbol.to_owned(),
                    symbol_path: symbol_path.to_owned(),
                });
            }
        }
    };
    for model in &ir.models {
        let base = format!("models.{}", model.name);
        let consumer = FacadeLeakKind::ConsumerSignature;
        match &model.render {
            ModelRenderSpec::Wrapper(spec) => {
                if let Some(constructor) = &spec.constructor {
                    for argument in &constructor.arguments {
                        check(
                            format!("{base}.new.{}", argument.name),
                            consumer.clone(),
                            &argument.type_name,
                        );
                    }
                }
                for factory in &spec.factories {
                    for argument in &factory.arguments {
                        check(
                            format!("{base}.{}.{}", factory.name, argument.name),
                            consumer.clone(),
                            &argument.type_name,
                        );
                    }
                }
                for setter in &spec.setters {
                    check(
                        format!("{base}.{}.{}", setter.name, setter.argument.name),
                        consumer.clone(),
                        &setter.argument.type_name,
                    );
                }
            }
            ModelRenderSpec::Union(spec) => {
                for branch in &spec.branches {
                    check(
                        format!("{base}.{}", branch.public_name),
                        consumer.clone(),
                        &branch.public_type,
                    );
                    check(
                        format!(
                            "{base}.{}.{}",
                            branch.constructor_name, branch.argument.name
                        ),
                        consumer.clone(),
                        &branch.argument.type_name,
                    );
                }
            }
            ModelRenderSpec::SimpleUnion(spec) => {
                for branch in &spec.branches {
                    check(
                        format!("{base}.{}", branch.public_name),
                        consumer.clone(),
                        &branch.public_type,
                    );
                }
            }
            ModelRenderSpec::View(spec) => {
                for accessor in &spec.accessors {
                    check(
                        format!("{base}.{}", accessor.name),
                        consumer.clone(),
                        &accessor.return_type,
                    );
                }
            }
            ModelRenderSpec::Alias(spec) => {
                check(base.clone(), consumer.clone(), &spec.public_type)
            }
            ModelRenderSpec::Map(spec) => {
                check(format!("{base}.new"), consumer.clone(), &spec.public_type);
                check(
                    format!("{base}.as_map"),
                    consumer.clone(),
                    &spec.public_type,
                );
                check(
                    format!("{base}.into_map"),
                    consumer.clone(),
                    &spec.public_type,
                );
            }
            ModelRenderSpec::ScalarEnum(_) => {}
        }
    }
    for resource in &ir.resources {
        for operation in &resource.operations {
            let base = format!("resources.{}.{}", resource.module, operation.name);
            if let Some(request) = &operation.parameter_request {
                for field in &request.fields {
                    check(
                        format!("{base}.request.{}", field.name),
                        FacadeLeakKind::ConsumerSignature,
                        &field.type_name,
                    );
                }
            } else {
                check(
                    format!("{base}.parameters"),
                    FacadeLeakKind::ConsumerSignature,
                    &operation.call.arguments,
                );
            }
            if let RequestProjection::Raw { raw_parameter, .. } = &operation.request_projection {
                check(
                    format!("{base}.request.{raw_parameter}"),
                    FacadeLeakKind::ConsumerSignature,
                    &operation.call.arguments,
                );
            }
            match &operation.response_projection {
                ResponseProjection::Json { model, .. } => check(
                    format!("{base}.response"),
                    FacadeLeakKind::ConsumerSignature,
                    model,
                ),
                ResponseProjection::BinaryBuffered { type_name } => check(
                    format!("{base}.response"),
                    FacadeLeakKind::ConsumerSignature,
                    type_name,
                ),
                ResponseProjection::Sse(stream) => {
                    check(
                        format!("{base}.stream"),
                        FacadeLeakKind::ConsumerSignature,
                        &stream.type_name,
                    );
                    if stream.variants.is_empty() {
                        check(
                            format!("{base}.stream.item"),
                            FacadeLeakKind::ConsumerSignature,
                            &stream.wrapper,
                        );
                    } else {
                        for variant in &stream.variants {
                            check(
                                format!("{base}.stream.{}", variant.name),
                                FacadeLeakKind::ConsumerSignature,
                                &variant.wrapper,
                            );
                        }
                    }
                }
                ResponseProjection::Empty
                | ResponseProjection::Text
                | ResponseProjection::Binary => {}
            }
        }
    }
    FacadeReport { leaks }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        ArgumentKind, ArgumentSpec, ModelSpec, SetterSpec, ValueSpec, WrapperModelSpec,
    };

    fn bindings() -> Bindings {
        serde_json::from_str(include_str!("../tests/fixtures/library/rust-bindings.json"))
            .expect("backend-neutral library bindings")
    }

    #[test]
    fn reports_closed_surface_without_public_transport_interop() {
        let ir = FacadeIr {
            client_name: "LibraryClient".into(),
            models: vec![],
            resources: vec![],
        };
        let report = inspect(&ir, &bindings());
        assert!(report.is_closed());
        report.require_closed().expect("closed facade");
    }

    #[test]
    fn finds_backend_symbols_in_nested_public_types_and_names_owner() {
        let bindings = bindings();
        let raw = bindings
            .structs
            .keys()
            .next()
            .expect("fixture struct")
            .clone();
        let type_name =
            format!("Option<Vec<std::collections::BTreeMap<String, Result<{raw}, ()>>>>");
        let ir = FacadeIr {
            client_name: "LibraryClient".into(),
            models: vec![ModelSpec {
                name: "Consumer".into(),
                raw: raw.clone(),
                render: ModelRenderSpec::Wrapper(WrapperModelSpec {
                    constructor: None,
                    factories: vec![],
                    setters: vec![SetterSpec {
                        name: "with_records".into(),
                        raw_field: "records".into(),
                        argument: ArgumentSpec {
                            name: "records".into(),
                            kind: ArgumentKind::Exact,
                            type_name: type_name.clone(),
                        },
                        value: ValueSpec::Variable("records".into()),
                        null_name: None,
                    }],
                    default: false,
                }),
            }],
            resources: vec![],
        };
        let report = inspect(&ir, &bindings);
        let leak = report.consumer_leaks().next().expect("nested leak");
        assert_eq!(leak.path, "models.Consumer.with_records.records");
        assert_eq!(leak.type_name, type_name);
        assert_eq!(leak.symbol, raw);
        assert_eq!(leak.symbol_path, bindings.symbol_paths[&raw]);
    }

    #[test]
    fn recognizes_qualified_paths_in_streams_tuples_and_arrays() {
        let paths =
            paths("Pin<Box<dyn Stream<Item = Result<(crate::generated::Raw, [Raw; 3]), E>>>>");
        assert!(paths.contains(&"crate::generated::Raw"));
        assert!(paths.contains(&"Raw"));
        assert!(!paths.contains(&"Stream<Item"));
    }
}

#[cfg(test)]
mod lowered_fixture_tests {
    use super::*;
    use crate::{OpenApi, Runtime};

    fn library() -> GenerateInput {
        GenerateInput {
            openapi: OpenApi(
                serde_json::from_str(include_str!("../tests/fixtures/library/openapi.json"))
                    .unwrap(),
            ),
            bindings: serde_json::from_str(include_str!(
                "../tests/fixtures/library/rust-bindings.json"
            ))
            .unwrap(),
            definition: serde_json::from_str(include_str!("../tests/fixtures/library/policy.json"))
                .unwrap(),
            runtime: Runtime::default(),
        }
    }

    #[test]
    fn backend_neutral_library_has_closed_public_facade() {
        let input = library();
        let report = inspect_public_facade(&input).expect("valid lowered fixture");
        assert!(report.is_closed());
    }

    #[test]
    fn backend_neutral_library_reports_unprojected_request_and_response() {
        let input = library();
        let mut ir = lower::lower(
            &input.openapi,
            &input.bindings,
            &input.definition,
            &input.runtime,
        )
        .unwrap();
        let operation = ir
            .resources
            .iter_mut()
            .flat_map(|resource| &mut resource.operations)
            .find(|operation| operation.name == "create")
            .unwrap();
        operation.call.arguments =
            "request: Option<Vec<crate::generated::types::CreateBookRequest>>".into();
        operation.response_projection = ResponseProjection::Json {
            model: "std::collections::BTreeMap<String, Vec<BookResponse>>".into(),
            raw: "BookResponse".into(),
        };
        let report = inspect(&ir, &input.bindings);
        assert!(
            report
                .leaks
                .iter()
                .any(|leak| leak.path.ends_with("create.parameters")
                    && leak.symbol == "CreateBookRequest")
        );
        assert!(report.leaks.iter().any(|leak| leak.path.ends_with("create.response") && leak.symbol == "BookResponse"));
        let error = report.require_closed().expect_err("incomplete facade");
        assert_eq!(error.diagnostic.code, "facade.raw_symbol_exposed");
    }

    #[test]
    fn backend_neutral_library_reports_enum_payload_and_stream_item() {
        let input = library();
        let mut ir = lower::lower(
            &input.openapi,
            &input.bindings,
            &input.definition,
            &input.runtime,
        )
        .unwrap();
        ir.models.push(crate::ir::ModelSpec {
            name: "Choice".into(),
            raw: "BookResponse".into(),
            render: ModelRenderSpec::SimpleUnion(crate::ir::SimpleUnionModelSpec {
                branches: vec![crate::ir::SimpleUnionBranchSpec {
                    raw_name: "Book".into(),
                    public_name: "Book".into(),
                    public_type: "Vec<Option<BookResponse>>".into(),
                    adapt_depth: None,
                }],
                bidirectional: false,
            }),
        });
        let operation = ir
            .resources
            .iter_mut()
            .flat_map(|resource| &mut resource.operations)
            .find(|operation| operation.name == "list")
            .unwrap();
        operation.response_projection = ResponseProjection::Sse(crate::ir::StreamPolicy {
            item: "RawEvent".into(),
            wrapper: "BookResponse".into(),
            type_name: "BookStream".into(),
            variants: vec![],
        });
        let report = inspect(&ir, &input.bindings);
        assert!(
            report
                .consumer_leaks()
                .any(|leak| leak.path == "models.Choice.Book" && leak.symbol == "BookResponse")
        );
        assert!(
            report.consumer_leaks().any(
                |leak| leak.path.ends_with("list.stream.item") && leak.symbol == "BookResponse"
            )
        );
    }
}
