use std::collections::BTreeSet;

use crate::contracts::{AccessorKindDefinition, Bindings, ModelDefinition, SdkDefinition};
use crate::error::{GenerationError, Result};
use crate::rust_type::{Type, TypeKind, parse_type};

fn invalid(path: impl Into<String>, message: impl Into<String>) -> GenerationError {
    GenerationError::at("contract.invalid", path, message)
}

fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn require_identifier(path: &str, value: &str) -> Result<()> {
    if valid_identifier(value) {
        Ok(())
    } else {
        Err(invalid(path, format!("expected identifier, got {value:?}")))
    }
}

fn require_nonempty(path: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        Err(invalid(path, "value must not be empty"))
    } else {
        Ok(())
    }
}

fn require_unique(path: &str, values: &[String]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(invalid(path, format!("duplicate value {value:?}")));
        }
    }
    Ok(())
}

impl Bindings {
    /// Validate the versioned backend-neutral sidecar independently of any SDK definition.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 2 {
            return Err(invalid(
                "bindings.schema_version",
                format!(
                    "unsupported bindings schema version {}",
                    self.schema_version
                ),
            ));
        }

        let declared: BTreeSet<_> = self
            .structs
            .keys()
            .chain(self.enums.keys())
            .chain(self.aliases.keys())
            .collect();
        for name in &declared {
            if !self.symbol_paths.contains_key(*name) {
                return Err(invalid(
                    format!("bindings.symbol_paths.{name}"),
                    format!("raw symbol {name} is missing a qualified symbol path"),
                ));
            }
        }

        for (name, fields) in &self.structs {
            require_nonempty(&format!("bindings.structs.{name}"), name)?;
            let mut seen = BTreeSet::new();
            for (index, field) in fields.iter().enumerate() {
                if !seen.insert(field.name.as_str()) {
                    return Err(invalid(
                        format!("bindings.structs.{name}[{index}].name"),
                        format!("duplicate raw field {}", field.name),
                    ));
                }
                parse_type(&field.type_name).map_err(|error| {
                    invalid(
                        format!("bindings.structs.{name}[{index}].type"),
                        error.to_string(),
                    )
                })?;
            }
        }
        for (name, variants) in &self.enums {
            let mut seen = BTreeSet::new();
            for (index, variant) in variants.iter().enumerate() {
                if !seen.insert(variant.name.as_str()) {
                    return Err(invalid(
                        format!("bindings.enums.{name}[{index}].name"),
                        format!("duplicate raw variant {}", variant.name),
                    ));
                }
                if let Some(payload) = &variant.payload {
                    parse_type(payload).map_err(|error| {
                        invalid(
                            format!("bindings.enums.{name}[{index}].payload"),
                            error.to_string(),
                        )
                    })?;
                }
            }
        }
        for (name, alias) in &self.aliases {
            parse_type(alias)
                .map_err(|error| invalid(format!("bindings.aliases.{name}"), error.to_string()))?;
        }
        for (key, operation) in &self.operations {
            require_nonempty(&format!("bindings.operations.{key}.name"), &operation.name)?;
            for (index, parameter) in operation.parameters.iter().enumerate() {
                parse_type(&parameter.type_name).map_err(|error| {
                    invalid(
                        format!("bindings.operations.{key}.parameters[{index}].type"),
                        error.to_string(),
                    )
                })?;
            }
            for (field, spelling) in [
                ("return_type", operation.return_type.as_str()),
                ("success_type", operation.success_type.as_str()),
            ] {
                parse_type(spelling).map_err(|error| {
                    invalid(
                        format!("bindings.operations.{key}.{field}"),
                        error.to_string(),
                    )
                })?;
            }
            if let Some(stream) = &operation.stream {
                parse_type(&stream.item_type).map_err(|error| {
                    invalid(
                        format!("bindings.operations.{key}.stream.item_type"),
                        error.to_string(),
                    )
                })?;
                parse_type(&stream.error_type).map_err(|error| {
                    invalid(
                        format!("bindings.operations.{key}.stream.error_type"),
                        error.to_string(),
                    )
                })?;
            }
        }
        require_unique(
            "bindings.binding.type_preludes",
            &self.binding.type_preludes,
        )?;
        Ok(())
    }

    pub(crate) fn fields(&self, name: &str) -> Result<&[crate::FieldBinding]> {
        self.structs.get(name).map(Vec::as_slice).ok_or_else(|| {
            GenerationError::new(
                "bindings.unknown_struct",
                format!("raw Rust struct not found: {name}"),
            )
        })
    }

    pub(crate) fn variants(&self, name: &str) -> Result<&[crate::VariantBinding]> {
        self.enums.get(name).map(Vec::as_slice).ok_or_else(|| {
            GenerationError::new(
                "bindings.unknown_enum",
                format!("raw Rust enum not found: {name}"),
            )
        })
    }

    pub(crate) fn operation(&self, name: &str) -> Result<&crate::OperationBinding> {
        self.operations.get(name).ok_or_else(|| {
            GenerationError::new(
                "bindings.unknown_operation",
                format!("raw Rust operation not found: {name}"),
            )
        })
    }

    pub(crate) fn parsed_alias(&self, name: &str) -> Result<Type> {
        let spelling = self.aliases.get(name).ok_or_else(|| {
            GenerationError::new(
                "bindings.unknown_alias",
                format!("raw Rust alias not found: {name}"),
            )
        })?;
        parse_type(spelling)
    }

    pub(crate) fn qualified_type(&self, spelling: &str) -> Result<String> {
        fn render(bindings: &Bindings, syntax: &Type) -> String {
            if syntax.kind == TypeKind::Generic {
                return format!(
                    "{}<{}>",
                    syntax.constructor.as_deref().expect("generic constructor"),
                    syntax
                        .arguments
                        .iter()
                        .map(|argument| render(bindings, argument))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            bindings
                .symbol_paths
                .get(&syntax.spelling)
                .cloned()
                .unwrap_or_else(|| syntax.spelling.clone())
        }
        Ok(render(self, &parse_type(spelling)?))
    }
}

fn model_schema_branch_matches(model: &ModelDefinition, branch: &str) -> bool {
    let has = |name: &str| match name {
        "union" => model.union.is_some(),
        "simple_union" => model.simple_union.is_some(),
        "type_alias" => model.type_alias.is_some(),
        "map" => model.map.is_some(),
        "scalar_enum" => model.scalar_enum.is_some(),
        "constructor" => model.constructor.is_some(),
        "union_factory" => model.union_factory.is_some(),
        "accessors" => model.accessors.is_some(),
        _ => false,
    };
    if !has(branch) {
        return false;
    }
    let forbidden: &[&str] = match branch {
        "union" => &[
            "scalar_enum",
            "map",
            "constructor",
            "union_factory",
            "accessors",
        ],
        "simple_union" => &[
            "scalar_enum",
            "union",
            "type_alias",
            "map",
            "constructor",
            "union_factory",
            "accessors",
        ],
        "type_alias" => &[
            "scalar_enum",
            "union",
            "simple_union",
            "map",
            "constructor",
            "union_factory",
            "accessors",
        ],
        "map" => &[
            "scalar_enum",
            "union",
            "simple_union",
            "type_alias",
            "constructor",
            "union_factory",
            "accessors",
        ],
        "scalar_enum" => &[
            "union",
            "simple_union",
            "type_alias",
            "map",
            "constructor",
            "union_factory",
            "accessors",
        ],
        "constructor" => &[
            "scalar_enum",
            "union",
            "simple_union",
            "type_alias",
            "map",
            "union_factory",
            "accessors",
        ],
        "union_factory" => &["scalar_enum", "union", "map", "constructor", "accessors"],
        "accessors" => &[
            "scalar_enum",
            "union",
            "map",
            "constructor",
            "union_factory",
        ],
        _ => unreachable!(),
    };
    forbidden.iter().all(|name| !has(name))
}

impl SdkDefinition {
    /// Validate the versioned explicit definition shape before semantic cross-checking.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 2 {
            return Err(invalid(
                "definition.schema_version",
                format!("unsupported SDK definition version {}", self.schema_version),
            ));
        }
        require_identifier("definition.client.name", &self.client.name)?;
        for (name, model) in &self.models {
            require_identifier(&format!("definition.models.{name}"), name)?;
            let matching = [
                "union",
                "simple_union",
                "type_alias",
                "map",
                "scalar_enum",
                "constructor",
                "union_factory",
                "accessors",
            ]
            .into_iter()
            .filter(|branch| model_schema_branch_matches(model, branch))
            .count();
            if matching != 1 {
                return Err(invalid(
                    format!("definition.models.{name}"),
                    "model must satisfy exactly one supported semantic shape",
                ));
            }
            if let Some(raw) = &model.raw {
                require_identifier(&format!("definition.models.{name}.raw"), raw)?;
            }
            for (field, values) in [
                ("constructor", model.constructor.as_deref()),
                ("exclude", model.exclude.as_deref()),
            ] {
                if let Some(values) = values {
                    require_unique(&format!("definition.models.{name}.{field}"), values)?;
                    for value in values {
                        require_identifier(&format!("definition.models.{name}.{field}"), value)?;
                    }
                }
            }
            if let Some(union) = &model.union {
                require_identifier(&format!("definition.models.{name}.union.root"), &union.root)?;
                if union.path.is_empty() {
                    return Err(invalid(
                        format!("definition.models.{name}.union.path"),
                        "path must not be empty",
                    ));
                }
                require_unique(
                    &format!("definition.models.{name}.union.targets"),
                    &union.targets,
                )?;
            }
            if let Some(simple) = &model.simple_union {
                if simple.variants.is_empty() {
                    return Err(invalid(
                        format!("definition.models.{name}.simple_union.variants"),
                        "variants must not be empty",
                    ));
                }
            }
            if let Some(factory) = &model.union_factory {
                require_identifier(
                    &format!("definition.models.{name}.union_factory.field"),
                    &factory.field,
                )?;
                require_unique(
                    &format!("definition.models.{name}.union_factory.leading"),
                    &factory.leading,
                )?;
            }
            if let Some(accessors) = &model.accessors {
                for (accessor_name, accessor) in accessors {
                    require_identifier(
                        &format!("definition.models.{name}.accessors.{accessor_name}"),
                        accessor_name,
                    )?;
                    if accessor.path.is_empty()
                        && accessor.kind != AccessorKindDefinition::Iter
                    {
                        return Err(invalid(
                            format!("definition.models.{name}.accessors.{accessor_name}.path"),
                            "path must not be empty except for a root iter accessor",
                        ));
                    }
                    if accessor.kind == AccessorKindDefinition::Iter && accessor.wrapper.is_none() {
                        return Err(invalid(
                            format!("definition.models.{name}.accessors.{accessor_name}.wrapper"),
                            "iter accessor requires a wrapper",
                        ));
                    }
                }
            }
        }

        for (module, resource) in &self.resources {
            require_identifier(&format!("definition.resources.{module}"), module)?;
            require_identifier(
                &format!("definition.resources.{module}.name"),
                &resource.name,
            )?;
            if let Some(path) = &resource.path {
                if path.is_empty() {
                    return Err(invalid(
                        format!("definition.resources.{module}.path"),
                        "path must not be empty",
                    ));
                }
                for segment in path {
                    require_identifier(&format!("definition.resources.{module}.path"), segment)?;
                }
            }
            for (name, operation) in &resource.operations {
                require_identifier(
                    &format!("definition.resources.{module}.operations.{name}"),
                    name,
                )?;
                require_nonempty(
                    &format!("definition.resources.{module}.operations.{name}.operation_id"),
                    &operation.operation_id,
                )?;
                let responses = usize::from(operation.response.is_some())
                    + usize::from(operation.stream.is_some())
                    + usize::from(operation.empty_response == Some(true))
                    + usize::from(operation.binary_response == Some(true));
                if responses != 1 {
                    return Err(invalid(
                        format!("definition.resources.{module}.operations.{name}"),
                        "operation must select exactly one response projection",
                    ));
                }
                if operation.request.is_none() && operation.request_overrides.is_some() {
                    return Err(invalid(
                        format!(
                            "definition.resources.{module}.operations.{name}.request_overrides"
                        ),
                        "request overrides require a request model",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_generic_fixtures() {
        for value in [
            include_str!("../tests/fixtures/menagerie/rust-bindings.json"),
            include_str!("../tests/fixtures/library/rust-bindings.json"),
        ] {
            let bindings: Bindings = serde_json::from_str(value).expect("fixture bindings");
            bindings.validate().expect("valid bindings");
        }
        for value in [
            include_str!("../tests/fixtures/menagerie/policy.json"),
            include_str!("../tests/fixtures/library/policy.json"),
        ] {
            let definition: SdkDefinition =
                serde_json::from_str(value).expect("fixture definition");
            definition.validate().expect("valid definition");
        }
    }

    #[test]
    fn qualifies_only_leaf_symbols() {
        let bindings: Bindings = serde_json::from_str(include_str!(
            "../tests/fixtures/menagerie/rust-bindings.json"
        ))
        .expect("fixture bindings");
        let qualified = bindings
            .qualified_type("Option<Vec<AnimalResponse>>")
            .expect("qualified type");
        assert!(qualified.starts_with("Option<Vec<"));
        assert!(qualified.contains("AnimalResponse"));
    }
}
