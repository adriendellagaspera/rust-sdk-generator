use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::contracts::OpenApi;
use crate::derivation::PublicSdkSurface;
use crate::error::{GenerationError, Result};

const ACTION_PREFIXES: &[&str] = &[
    "get", "list", "create", "update", "delete", "post", "put", "patch", "start", "cancel",
    "judge", "execute", "archive", "export", "import",
];

const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NamingDecision {
    pub public_path: Option<String>,
    pub evidence: Vec<String>,
    pub reason: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct OperationNamingInput {
    operation_id: String,
    route: String,
    tags: Vec<String>,
}

fn error(code: &'static str, message: impl Into<String>) -> GenerationError {
    GenerationError::new(code, message)
}

fn identifier(value: &str) -> String {
    let mut result = String::new();
    let mut underscore = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if ch == '_' {
                if !result.is_empty() {
                    underscore = true;
                }
            } else {
                if underscore && !result.is_empty() {
                    result.push('_');
                }
                underscore = false;
                result.push(ch.to_ascii_lowercase());
            }
        } else if !result.is_empty() {
            underscore = true;
        }
    }
    result.trim_matches('_').to_owned()
}

fn valid_public_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !RUST_KEYWORDS.contains(&value)
}

fn operation_stem(operation_id: &str) -> String {
    let normalized = identifier(operation_id);
    let parts: Vec<_> = normalized.split('_').collect();
    let version = parts.iter().position(|part| {
        part.strip_prefix('v').is_some_and(|digits| {
            !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit())
        })
    });
    match version {
        Some(0) | None => normalized,
        Some(index) => parts[..index].join("_"),
    }
}

fn common_operation_prefix(operation_ids: &[String]) -> Vec<String> {
    if operation_ids.len() < 2 {
        return Vec::new();
    }
    let split: Vec<Vec<_>> = operation_ids
        .iter()
        .map(|operation_id| {
            operation_stem(operation_id)
                .split('_')
                .map(str::to_owned)
                .collect()
        })
        .collect();
    let mut prefix = Vec::new();
    let shortest = split.iter().map(Vec::len).min().unwrap_or(0);
    for index in 0..shortest {
        let first = &split[0][index];
        if split.iter().all(|parts| &parts[index] == first) {
            prefix.push(first.clone());
        } else {
            break;
        }
    }
    if prefix
        .first()
        .is_some_and(|first| ACTION_PREFIXES.contains(&first.as_str()))
    {
        Vec::new()
    } else {
        prefix
    }
}

fn canonical_surface_path(paths: &[String]) -> NamingDecision {
    if paths.is_empty() {
        return NamingDecision {
            public_path: None,
            evidence: Vec::new(),
            reason: None,
        };
    }
    let mut evidence = paths.to_vec();
    evidence.sort();
    evidence.dedup();
    let parsed: Vec<_> = evidence
        .iter()
        .map(|path| path.split('.').collect::<Vec<_>>())
        .collect();
    if parsed
        .iter()
        .any(|parts| parts.len() < 2 || parts.iter().any(|part| !valid_public_identifier(part)))
    {
        return NamingDecision {
            public_path: None,
            evidence,
            reason: Some("surface.invalid_public_path"),
        };
    }
    let depth = parsed.iter().map(Vec::len).max().unwrap_or(0);
    let canonical = evidence
        .iter()
        .zip(parsed.iter())
        .filter(|(_, parts)| parts.len() == depth)
        .map(|(path, _)| path)
        .next()
        .expect("non-empty explicit surface paths")
        .clone();
    NamingDecision {
        public_path: Some(canonical),
        evidence,
        reason: None,
    }
}

fn operations(openapi: &OpenApi) -> Result<BTreeMap<String, OperationNamingInput>> {
    let root = openapi
        .0
        .as_object()
        .ok_or_else(|| error("openapi.invalid", "OpenAPI document must be a JSON object"))?;
    let mut result = BTreeMap::new();
    let Some(paths) = root.get("paths").and_then(Value::as_object) else {
        return Ok(result);
    };
    for (route, path_item) in paths {
        let Some(path_item) = path_item.as_object() else {
            continue;
        };
        for method in [
            "get", "put", "post", "delete", "patch", "head", "options", "trace",
        ] {
            let Some(operation) = path_item.get(method).and_then(Value::as_object) else {
                continue;
            };
            let Some(operation_id) = operation.get("operationId").and_then(Value::as_str) else {
                continue;
            };
            let tags = operation
                .get("tags")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            result.insert(
                operation_id.to_owned(),
                OperationNamingInput {
                    operation_id: operation_id.to_owned(),
                    route: route.clone(),
                    tags,
                },
            );
        }
    }
    Ok(result)
}

fn resource_path_from_route(route: &str) -> Option<Vec<String>> {
    let parts: Vec<_> = route
        .split('/')
        .filter(|part| !part.is_empty() && !(part.starts_with('{') && part.ends_with('}')))
        .map(identifier)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() || parts.iter().any(|part| !valid_public_identifier(part)) {
        None
    } else {
        Some(parts)
    }
}

fn fallback_resources_from_surface(
    operations: &BTreeMap<String, OperationNamingInput>,
    explicit: &BTreeMap<String, NamingDecision>,
) -> BTreeMap<String, BTreeSet<Vec<String>>> {
    let mut resources: BTreeMap<String, BTreeSet<Vec<String>>> = BTreeMap::new();
    for (operation_id, input) in operations {
        let Some(path) = explicit
            .get(operation_id)
            .and_then(|decision| decision.public_path.as_ref())
        else {
            continue;
        };
        let mut parts: Vec<_> = path.split('.').map(str::to_owned).collect();
        parts.pop();
        for tag in &input.tags {
            resources
                .entry(tag.clone())
                .or_default()
                .insert(parts.clone());
        }
    }
    resources
}

fn fallback_resource_for_tag(
    tag: &str,
    resources: &BTreeMap<String, BTreeSet<Vec<String>>>,
) -> Result<Option<Vec<String>>> {
    let tag_parts: Vec<_> = tag.split('.').collect();
    for depth in (1..=tag_parts.len()).rev() {
        let parent = tag_parts[..depth].join(".");
        if let Some(candidates) = resources.get(&parent) {
            if candidates.len() != 1 {
                return Err(error(
                    "surface.fallback_ambiguity",
                    format!("tag {tag} maps to multiple public resource paths"),
                ));
            }
            let mut path = candidates.iter().next().expect("one candidate").clone();
            for suffix in &tag_parts[depth..] {
                let suffix = identifier(suffix);
                if suffix.is_empty() || !valid_public_identifier(&suffix) {
                    return Ok(None);
                }
                path.push(suffix);
            }
            return Ok(Some(path));
        }
    }
    let normalized: Vec<_> = tag_parts.into_iter().map(identifier).collect();
    if normalized.is_empty()
        || normalized
            .iter()
            .any(|part| part.is_empty() || !valid_public_identifier(part))
    {
        Ok(None)
    } else {
        Ok(Some(normalized))
    }
}

fn fallback_decision(
    input: &OperationNamingInput,
    resources: &BTreeMap<String, BTreeSet<Vec<String>>>,
    prefixes: &BTreeMap<String, Vec<String>>,
) -> NamingDecision {
    let mut candidates = BTreeSet::new();
    for tag in &input.tags {
        let resource = match fallback_resource_for_tag(tag, resources) {
            Ok(Some(resource)) => resource,
            Ok(None) => {
                return NamingDecision {
                    public_path: None,
                    evidence: Vec::new(),
                    reason: Some("surface.invalid_fallback"),
                };
            }
            Err(_) => {
                return NamingDecision {
                    public_path: None,
                    evidence: Vec::new(),
                    reason: Some("surface.fallback_ambiguity"),
                };
            }
        };
        let mut method: Vec<_> = operation_stem(&input.operation_id)
            .split('_')
            .map(str::to_owned)
            .collect();
        let tag_parts: Vec<_> = tag.split('.').map(identifier).collect();
        let tag_prefix_len = tag_parts
            .iter()
            .zip(method.iter())
            .take_while(|(tag_part, method_part)| tag_part == method_part)
            .count();
        let prefix_len = prefixes
            .get(tag)
            .filter(|prefix| !prefix.is_empty() && method.starts_with(prefix))
            .map_or(tag_prefix_len, Vec::len);
        if prefix_len > 0 && method.len() > prefix_len {
            method.drain(..prefix_len);
        }
        let method = method.join("_");
        if method.is_empty() || !valid_public_identifier(&method) {
            return NamingDecision {
                public_path: None,
                evidence: Vec::new(),
                reason: Some("surface.invalid_fallback"),
            };
        }
        candidates.insert(
            resource
                .iter()
                .chain(std::iter::once(&method))
                .cloned()
                .collect::<Vec<_>>()
                .join("."),
        );
    }

    if candidates.is_empty() {
        let Some(resource) = resource_path_from_route(&input.route) else {
            return NamingDecision {
                public_path: None,
                evidence: Vec::new(),
                reason: Some("surface.missing_public_naming_evidence"),
            };
        };
        let method = operation_stem(&input.operation_id);
        if method.is_empty() || !valid_public_identifier(&method) {
            return NamingDecision {
                public_path: None,
                evidence: Vec::new(),
                reason: Some("surface.invalid_fallback"),
            };
        }
        candidates.insert(
            resource
                .iter()
                .chain(std::iter::once(&method))
                .cloned()
                .collect::<Vec<_>>()
                .join("."),
        );
    }

    let evidence: Vec<_> = candidates.into_iter().collect();
    if evidence.len() != 1 {
        NamingDecision {
            public_path: None,
            evidence,
            reason: Some("surface.fallback_ambiguity"),
        }
    } else {
        NamingDecision {
            public_path: Some(evidence[0].clone()),
            evidence,
            reason: None,
        }
    }
}

pub(crate) fn derive_public_paths(
    openapi: &OpenApi,
    surface: &PublicSdkSurface,
) -> Result<BTreeMap<String, NamingDecision>> {
    let operations = operations(openapi)?;
    let explicit: BTreeMap<_, _> = operations
        .keys()
        .map(|operation_id| {
            (
                operation_id.clone(),
                canonical_surface_path(
                    surface
                        .operations
                        .get(operation_id)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                ),
            )
        })
        .collect();
    let resources = fallback_resources_from_surface(&operations, &explicit);
    let mut operation_ids_by_tag: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for input in operations.values() {
        for tag in &input.tags {
            operation_ids_by_tag
                .entry(tag.clone())
                .or_default()
                .push(input.operation_id.clone());
        }
    }
    let prefixes: BTreeMap<_, _> = operation_ids_by_tag
        .into_iter()
        .map(|(tag, operation_ids)| (tag, common_operation_prefix(&operation_ids)))
        .collect();

    Ok(operations
        .into_iter()
        .map(|(operation_id, input)| {
            let decision = explicit[&operation_id].clone();
            let decision = if decision.public_path.is_none() && decision.reason.is_none() {
                fallback_decision(&input, &resources, &prefixes)
            } else {
                decision
            };
            (operation_id, decision)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(entries: &[(&str, &[&str])]) -> PublicSdkSurface {
        PublicSdkSurface {
            schema_version: 1,
            client: None,
            operations: entries
                .iter()
                .map(|(operation, paths)| {
                    (
                        (*operation).to_owned(),
                        paths.iter().map(|path| (*path).to_owned()).collect(),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn deepest_explicit_path_wins_without_transport_guessing() {
        let api = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {"/events": {"get": {"operationId": "events", "responses": {"204": {}}}}}
        }));
        let decisions = derive_public_paths(
            &api,
            &surface(&[("events", &["events.list", "admin.events.list"])]),
        )
        .expect("naming");
        assert_eq!(
            decisions["events"].public_path.as_deref(),
            Some("admin.events.list")
        );
    }

    #[test]
    fn equal_depth_explicit_aliases_have_a_deterministic_canonical_path() {
        let api = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {"/events": {"get": {"operationId": "events", "responses": {"204": {}}}}}
        }));
        let decisions = derive_public_paths(
            &api,
            &surface(&[("events", &["events.list", "events.list_stream"])]),
        )
        .expect("naming");
        assert_eq!(
            decisions["events"].public_path.as_deref(),
            Some("events.list")
        );
        assert_eq!(
            decisions["events"].evidence,
            vec!["events.list", "events.list_stream"]
        );
        assert_eq!(decisions["events"].reason, None);
    }

    #[test]
    fn incomplete_surface_uses_tag_parent_evidence_and_operation_prefix() {
        let api = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/jobs": {"get": {"operationId": "jobs_list", "tags": ["jobs"], "responses": {"204": {}}}},
                "/jobs/{id}": {"delete": {"operationId": "jobs_delete", "tags": ["jobs.admin"], "responses": {"204": {}}}}
            }
        }));
        let decisions = derive_public_paths(&api, &surface(&[("jobs_list", &["work.jobs.list"])]))
            .expect("naming");
        assert_eq!(
            decisions["jobs_delete"].public_path.as_deref(),
            Some("work.jobs.admin.delete")
        );
    }

    #[test]
    fn route_fallback_is_deterministic_without_surface_or_tags() {
        let api = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {"/warehouse/items/{id}": {"get": {"operationId": "read_item_v2", "responses": {"204": {}}}}}
        }));
        let decisions = derive_public_paths(&api, &PublicSdkSurface::default()).expect("naming");
        assert_eq!(
            decisions["read_item_v2"].public_path.as_deref(),
            Some("warehouse.items.read_item")
        );
    }
}
