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
        if !matches!(self.schema_version, 2..=5) {
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
                require_nonempty(
                    &format!("bindings.structs.{name}[{index}].name"),
                    &field.name,
                )?;
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
                if self.schema_version == 5 && field.serialized_presence.is_none() {
                    return Err(invalid(
                        format!("bindings.structs.{name}[{index}].serialized_presence"),
                        "Bindings v5 fields require explicit serialized-presence evidence",
                    ));
                }
            }
        }
        for (name, variants) in &self.enums {
            let mut seen = BTreeSet::new();
            for (index, variant) in variants.iter().enumerate() {
                require_nonempty(
                    &format!("bindings.enums.{name}[{index}].name"),
                    &variant.name,
                )?;
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

        let mut canonical_operations = BTreeSet::new();
        for (key, operation) in &self.operations {
            require_nonempty(&format!("bindings.operations.{key}.name"), &operation.name)?;
            for (index, parameter) in operation.parameters.iter().enumerate() {
                require_nonempty(
                    &format!("bindings.operations.{key}.parameters[{index}].name"),
                    &parameter.name,
                )?;
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
                require_nonempty(
                    &format!("bindings.operations.{key}.stream.lifetime"),
                    &stream.lifetime,
                )?;
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

            match (self.schema_version, &operation.metadata) {
                (2, None) => {}
                (2, Some(_)) => {
                    return Err(invalid(
                        format!("bindings.operations.{key}.metadata"),
                        "operation metadata requires Bindings schema version 3, 4, or 5",
                    ));
                }
                (3..=5, None) => {
                    return Err(invalid(
                        format!("bindings.operations.{key}.metadata"),
                        format!(
                            "Bindings v{} operation is missing canonical identity metadata",
                            self.schema_version
                        ),
                    ));
                }
                (3..=5, Some(metadata)) => {
                    if operation.name != *key {
                        return Err(invalid(
                            format!("bindings.operations.{key}.name"),
                            format!(
                                "Bindings v{} operation name must equal its raw Rust method key",
                                self.schema_version
                            ),
                        ));
                    }
                    require_nonempty(
                        &format!(
                            "bindings.operations.{key}.metadata.source_operation.operation_id"
                        ),
                        &metadata.source_operation.operation_id,
                    )?;
                    require_nonempty(
                        &format!("bindings.operations.{key}.metadata.source_operation.method"),
                        &metadata.source_operation.method,
                    )?;
                    require_nonempty(
                        &format!("bindings.operations.{key}.metadata.source_operation.path"),
                        &metadata.source_operation.path,
                    )?;
                    require_nonempty(
                        &format!("bindings.operations.{key}.metadata.emitted_operation_id"),
                        &metadata.emitted_operation_id,
                    )?;
                    require_unique(
                        &format!("bindings.operations.{key}.metadata.success_statuses"),
                        &metadata.success_statuses,
                    )?;

                    match &metadata.representation {
                        crate::ResponseRepresentationBinding::Json {
                            schema_name,
                            media_type,
                        } => {
                            require_nonempty(
                                &format!(
                                    "bindings.operations.{key}.metadata.representation.schema_name"
                                ),
                                schema_name,
                            )?;
                            require_nonempty(
                                &format!(
                                    "bindings.operations.{key}.metadata.representation.media_type"
                                ),
                                media_type,
                            )?;
                        }
                        crate::ResponseRepresentationBinding::Text { media_type }
                        | crate::ResponseRepresentationBinding::EventStream { media_type }
                        | crate::ResponseRepresentationBinding::BinaryBuffered {
                            media_type, ..
                        }
                        | crate::ResponseRepresentationBinding::BinaryStream {
                            media_type, ..
                        } => {
                            require_nonempty(
                                &format!(
                                    "bindings.operations.{key}.metadata.representation.media_type"
                                ),
                                media_type,
                            )?;
                        }
                        crate::ResponseRepresentationBinding::Empty => {}
                    }

                    let mut parameter_names = BTreeSet::new();
                    let mut parameter_wires = BTreeSet::new();
                    for (index, wire) in metadata.parameter_wires.iter().enumerate() {
                        let context =
                            format!("bindings.operations.{key}.metadata.parameter_wires[{index}]");
                        require_nonempty(&format!("{context}.rust_name"), &wire.rust_name)?;
                        require_nonempty(&format!("{context}.wire_name"), &wire.wire_name)?;
                        if !matches!(wire.location.as_str(), "query" | "header") {
                            return Err(invalid(
                                format!("{context}.location"),
                                "expected query or header",
                            ));
                        }
                        let canonical_wire = if wire.location == "header" {
                            wire.wire_name.to_ascii_lowercase()
                        } else {
                            wire.wire_name.clone()
                        };
                        if !parameter_names.insert(wire.rust_name.as_str())
                            || !parameter_wires.insert((wire.location.as_str(), canonical_wire))
                        {
                            return Err(invalid(
                                context,
                                "duplicate Rust parameter or HTTP wire key",
                            ));
                        }
                    }

                    for (index, discriminator) in metadata.request_discriminators.iter().enumerate()
                    {
                        let context = format!(
                            "bindings.operations.{key}.metadata.request_discriminators[{index}]"
                        );
                        require_nonempty(
                            &format!("{context}.wire_name"),
                            &discriminator.wire_name,
                        )?;
                        if discriminator.rust_access_path.is_empty()
                            || discriminator
                                .rust_access_path
                                .iter()
                                .any(|segment| segment.is_empty())
                        {
                            return Err(invalid(
                                format!("{context}.rust_access_path"),
                                "must contain only non-empty path segments",
                            ));
                        }
                        parse_type(&discriminator.rust_value_type).map_err(|error| {
                            invalid(format!("{context}.rust_value_type"), error.to_string())
                        })?;
                    }

                    let representation_streams = matches!(
                        metadata.representation,
                        crate::ResponseRepresentationBinding::EventStream { .. }
                            | crate::ResponseRepresentationBinding::BinaryStream { .. }
                    );
                    if representation_streams != operation.stream.is_some() {
                        return Err(invalid(
                            format!("bindings.operations.{key}.stream"),
                            "common stream view must match response representation",
                        ));
                    }

                    match self.schema_version {
                        3 => {
                            if metadata.stream_transport.is_some() {
                                return Err(invalid(
                                    format!("bindings.operations.{key}.metadata.stream_transport"),
                                    "Bindings v3 cannot contain a v4 stream transport",
                                ));
                            }
                            if representation_streams != metadata.stream_abi.is_some() {
                                return Err(invalid(
                                    format!("bindings.operations.{key}.metadata.stream_abi"),
                                    "stream ABI presence must match response representation",
                                ));
                            }
                            if let Some(abi) = &metadata.stream_abi {
                                for (field, spelling) in [
                                    ("item_type", abi.item_type.as_str()),
                                    ("error_type", abi.error_type.as_str()),
                                    ("native_type", abi.native_type.as_str()),
                                    ("wasm_type", abi.wasm_type.as_str()),
                                ] {
                                    parse_type(spelling).map_err(|error| {
                                        invalid(
                                            format!(
                                                "bindings.operations.{key}.metadata.stream_abi.{field}"
                                            ),
                                            error.to_string(),
                                        )
                                    })?;
                                }
                                require_nonempty(
                                    &format!("bindings.operations.{key}.metadata.stream_abi.alias"),
                                    &abi.alias,
                                )?;
                                require_nonempty(
                                    &format!(
                                        "bindings.operations.{key}.metadata.stream_abi.lifetime"
                                    ),
                                    &abi.lifetime,
                                )?;
                                let stream =
                                    operation.stream.as_ref().expect("checked stream presence");
                                if stream.item_type != abi.item_type
                                    || stream.error_type != abi.error_type
                                    || stream.lifetime != abi.lifetime
                                {
                                    return Err(invalid(
                                        format!("bindings.operations.{key}.metadata.stream_abi"),
                                        "stream ABI common view disagrees with operation.stream",
                                    ));
                                }
                            }
                        }
                        4 | 5 => {
                            if metadata.stream_abi.is_some() {
                                return Err(invalid(
                                    format!("bindings.operations.{key}.metadata.stream_abi"),
                                    "Bindings v4/v5 use stream_transport instead of stream_abi",
                                ));
                            }
                            if representation_streams != metadata.stream_transport.is_some() {
                                return Err(invalid(
                                    format!("bindings.operations.{key}.metadata.stream_transport"),
                                    "stream transport presence must match response representation",
                                ));
                            }
                            if let Some(transport) = &metadata.stream_transport {
                                match transport {
                                    crate::StreamTransportBinding::NamedAlias {
                                        alias,
                                        native_type,
                                        wasm_type,
                                    } => {
                                        require_nonempty(
                                            &format!(
                                                "bindings.operations.{key}.metadata.stream_transport.alias"
                                            ),
                                            alias,
                                        )?;
                                        for (field, spelling) in [
                                            ("native_type", native_type.as_str()),
                                            ("wasm_type", wasm_type.as_str()),
                                        ] {
                                            parse_type(spelling).map_err(|error| {
                                                invalid(
                                                    format!(
                                                        "bindings.operations.{key}.metadata.stream_transport.{field}"
                                                    ),
                                                    error.to_string(),
                                                )
                                            })?;
                                        }
                                    }
                                    crate::StreamTransportBinding::AnonymousImplTrait {
                                        rust_type,
                                    } => {
                                        parse_type(rust_type).map_err(|error| {
                                            invalid(
                                                format!(
                                                    "bindings.operations.{key}.metadata.stream_transport.rust_type"
                                                ),
                                                error.to_string(),
                                            )
                                        })?;
                                    }
                                }
                            }
                        }
                        _ => unreachable!("schema version validated above"),
                    }

                    let identity = (
                        metadata.kind.clone(),
                        metadata.source_operation.clone(),
                        metadata.representation.clone(),
                    );
                    if !canonical_operations.insert(identity) {
                        return Err(invalid(
                            format!("bindings.operations.{key}.metadata"),
                            "duplicate canonical source-operation/representation identity",
                        ));
                    }
                }
                _ => unreachable!("schema version validated above"),
            }
        }
        for (field, value) in [
            ("type_path", self.binding.client.type_path.as_str()),
            ("constructor", self.binding.client.constructor.as_str()),
            (
                "api_key_builder",
                self.binding.client.api_key_builder.as_str(),
            ),
            (
                "base_url_builder",
                self.binding.client.base_url_builder.as_str(),
            ),
        ] {
            require_nonempty(&format!("bindings.binding.client.{field}"), value)?;
        }
        for (index, prelude) in self.binding.type_preludes.iter().enumerate() {
            require_nonempty(&format!("bindings.binding.type_preludes[{index}]"), prelude)?;
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
                    if accessor.path.is_empty() && accessor.kind != AccessorKindDefinition::Iter {
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
                let legacy_responses = usize::from(operation.response.is_some())
                    + usize::from(operation.stream.is_some())
                    + usize::from(operation.empty_response == Some(true))
                    + usize::from(operation.binary_response == Some(true));
                if let Some(representation) = operation.response_representation {
                    let compatible = match representation {
                        crate::ResponseRepresentationDefinition::Json => {
                            operation.response.is_some() && legacy_responses == 1
                        }
                        crate::ResponseRepresentationDefinition::Empty => {
                            operation.empty_response == Some(true) && legacy_responses == 1
                        }
                        crate::ResponseRepresentationDefinition::Text
                        | crate::ResponseRepresentationDefinition::BinaryBuffered => {
                            legacy_responses == 0
                        }
                        crate::ResponseRepresentationDefinition::EventStream => {
                            operation.stream.is_some() && legacy_responses == 1
                        }
                        crate::ResponseRepresentationDefinition::BinaryStream => {
                            operation.binary_response == Some(true) && legacy_responses == 1
                        }
                    };
                    if !compatible {
                        return Err(invalid(
                            format!(
                                "definition.resources.{module}.operations.{name}.response_representation"
                            ),
                            "response representation disagrees with configured response projection",
                        ));
                    }
                } else if legacy_responses != 1 {
                    return Err(invalid(
                        format!("definition.resources.{module}.operations.{name}"),
                        "operation must select exactly one response projection",
                    ));
                }
                if let Some(stream) = &operation.stream {
                    parse_type(&stream.item).map_err(|error| {
                        invalid(
                            format!("definition.resources.{module}.operations.{name}.stream.item"),
                            error.to_string(),
                        )
                    })?;
                    require_identifier(
                        &format!("definition.resources.{module}.operations.{name}.stream.type"),
                        &stream.type_name,
                    )?;
                    if let Some(wrapper) = &stream.wrapper {
                        require_identifier(
                            &format!(
                                "definition.resources.{module}.operations.{name}.stream.wrapper"
                            ),
                            wrapper,
                        )?;
                    }
                    if !stream.variants.is_empty() {
                        if stream.variants.len() < 2 || stream.wrapper.is_none() {
                            return Err(invalid(
                                format!(
                                    "definition.resources.{module}.operations.{name}.stream.variants"
                                ),
                                "typed SSE union requires at least two variants and a public wrapper",
                            ));
                        }
                        let mut variant_names = BTreeSet::new();
                        let mut schemas = BTreeSet::new();
                        let mut raws = BTreeSet::new();
                        let mut wrappers = BTreeSet::new();
                        for (index, variant) in stream.variants.iter().enumerate() {
                            let context = format!(
                                "definition.resources.{module}.operations.{name}.stream.variants[{index}]"
                            );
                            require_identifier(&format!("{context}.name"), &variant.name)?;
                            require_nonempty(&format!("{context}.schema"), &variant.schema)?;
                            parse_type(&variant.raw).map_err(|error| {
                                invalid(format!("{context}.raw"), error.to_string())
                            })?;
                            require_identifier(&format!("{context}.wrapper"), &variant.wrapper)?;
                            if !variant_names.insert(&variant.name)
                                || !schemas.insert(&variant.schema)
                                || !raws.insert(&variant.raw)
                                || !wrappers.insert(&variant.wrapper)
                            {
                                return Err(invalid(
                                    context,
                                    "typed SSE union variants must be bijectively unique",
                                ));
                            }
                        }
                    }
                }
                if let Some(media) = operation.request_media {
                    let structured = matches!(
                        media,
                        crate::RequestMediaDefinition::Json
                            | crate::RequestMediaDefinition::MultipartFormData
                            | crate::RequestMediaDefinition::FormUrlencoded
                    );
                    if structured != operation.request.is_some() {
                        return Err(invalid(
                            format!(
                                "definition.resources.{module}.operations.{name}.request_media"
                            ),
                            if structured {
                                "structured request media requires a request model"
                            } else {
                                "raw request media must not use a facade request model"
                            },
                        ));
                    }
                }
                if operation.request.is_none() && operation.request_overrides.is_some() {
                    return Err(invalid(
                        format!(
                            "definition.resources.{module}.operations.{name}.request_overrides"
                        ),
                        "request overrides require a request model",
                    ));
                }
                for (raw_name, adapter) in &operation.parameter_adapters {
                    let context = format!(
                        "definition.resources.{module}.operations.{name}.parameter_adapters.{raw_name}"
                    );
                    require_nonempty(&context, raw_name)?;
                    require_identifier(&format!("{context}.model"), &adapter.model)?;
                    if !matches!(adapter.location.as_str(), "path" | "query" | "header") {
                        return Err(invalid(
                            format!("{context}.location"),
                            "parameter adapter location must be path, query, or header",
                        ));
                    }
                    require_nonempty(&format!("{context}.wire_name"), &adapter.wire_name)?;
                }
                if let Some(enabled) = operation.multipart_filenames {
                    if !enabled {
                        return Err(invalid(
                            format!(
                                "definition.resources.{module}.operations.{name}.multipart_filenames"
                            ),
                            "multipart_filenames must be true when present",
                        ));
                    }
                    if operation.request.is_none()
                        || operation.request_media
                            != Some(crate::RequestMediaDefinition::MultipartFormData)
                    {
                        return Err(invalid(
                            format!(
                                "definition.resources.{module}.operations.{name}.multipart_filenames"
                            ),
                            "multipart filenames require a structured multipart request model",
                        ));
                    }
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
    fn bindings_v3_requires_unique_canonical_operation_identity() {
        let operation = serde_json::json!({
            "name": "health",
            "parameters": [],
            "return_type": "Result<(), Error>",
            "success_type": "()",
            "metadata": {
                "kind": "call_shape",
                "source_operation": {
                    "operation_id": "healthCheck",
                    "method": "GET",
                    "path": "/health"
                },
                "emitted_operation_id": "healthCheck",
                "representation": {"kind": "empty"},
                "success_statuses": ["204"],
                "request_discriminators": [],
                "stream_abi": null
            }
        });
        let mut value = serde_json::json!({
            "schema_version": 3,
            "structs": {},
            "enums": {},
            "aliases": {},
            "operations": {"health": operation},
            "symbol_paths": {},
            "binding": {
                "client": {
                    "type_path": "crate::generated::client::HttpClient",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": ["crate::generated::types::*"]
            }
        });
        let bindings: Bindings =
            serde_json::from_value(value.clone()).expect("deserialize Bindings v3");
        bindings.validate().expect("valid Bindings v3");

        value["operations"]["health_2"] = value["operations"]["health"].clone();
        value["operations"]["health_2"]["name"] = serde_json::json!("health_2");
        let duplicate: Bindings =
            serde_json::from_value(value).expect("deserialize duplicate Bindings v3");
        let error = duplicate
            .validate()
            .expect_err("canonical identity collision must fail");
        assert_eq!(error.diagnostic.code, "contract.invalid");
        assert!(
            error
                .diagnostic
                .message
                .contains("duplicate canonical source-operation/representation identity")
        );
    }

    #[test]
    fn bindings_v4_accepts_anonymous_owned_stream_transport() {
        let value = serde_json::json!({
            "schema_version": 4,
            "structs": {},
            "enums": {},
            "aliases": {},
            "operations": {
                "events": {
                    "name": "events",
                    "parameters": [],
                    "return_type": "Result<impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + 'static + use<>, Error>",
                    "success_type": "impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + 'static + use<>",
                    "stream": {
                        "item_type": "bytes::Bytes",
                        "error_type": "reqwest::Error",
                        "lifetime": "'static"
                    },
                    "metadata": {
                        "kind": "call_shape",
                        "source_operation": {
                            "operation_id": "events",
                            "method": "GET",
                            "path": "/events"
                        },
                        "emitted_operation_id": "events",
                        "representation": {
                            "kind": "event_stream",
                            "media_type": "text/event-stream"
                        },
                        "success_statuses": ["200"],
                        "request_discriminators": [],
                        "stream_transport": {
                            "kind": "anonymous_impl_trait",
                            "rust_type": "impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + 'static + use<>"
                        }
                    }
                }
            },
            "symbol_paths": {},
            "binding": {
                "client": {
                    "type_path": "crate::generated::client::HttpClient",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": []
            }
        });
        let bindings: Bindings = serde_json::from_value(value).expect("deserialize Bindings v4");
        bindings.validate().expect("valid anonymous v4 stream");
    }

    #[test]
    fn bindings_versions_reject_mixed_stream_contracts() {
        let mut value = serde_json::json!({
            "schema_version": 4,
            "structs": {},
            "enums": {},
            "aliases": {},
            "operations": {
                "events": {
                    "name": "events",
                    "parameters": [],
                    "return_type": "Result<HttpResponseByteStream, Error>",
                    "success_type": "HttpResponseByteStream",
                    "stream": {
                        "item_type": "bytes::Bytes",
                        "error_type": "reqwest::Error",
                        "lifetime": "'static"
                    },
                    "metadata": {
                        "kind": "call_shape",
                        "source_operation": {
                            "operation_id": "events",
                            "method": "GET",
                            "path": "/events"
                        },
                        "emitted_operation_id": "events",
                        "representation": {
                            "kind": "event_stream",
                            "media_type": "text/event-stream"
                        },
                        "success_statuses": ["200"],
                        "request_discriminators": [],
                        "stream_abi": {
                            "alias": "HttpResponseByteStream",
                            "item_type": "bytes::Bytes",
                            "error_type": "reqwest::Error",
                            "lifetime": "'static",
                            "native_type": "futures_util::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>",
                            "wasm_type": "futures_util::stream::LocalBoxStream<'static, Result<bytes::Bytes, reqwest::Error>>"
                        }
                    }
                }
            },
            "symbol_paths": {},
            "binding": {
                "client": {
                    "type_path": "crate::generated::client::HttpClient",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": []
            }
        });
        let mixed_v4 =
            serde_json::from_value::<Bindings>(value.clone()).expect("deserialize mixed v4");
        assert!(mixed_v4.validate().is_err());

        value["schema_version"] = serde_json::json!(3);
        value["operations"]["events"]["metadata"]["stream_transport"] = serde_json::json!({
            "kind": "named_alias",
            "alias": "HttpResponseByteStream",
            "native_type": "futures_util::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>",
            "wasm_type": "futures_util::stream::LocalBoxStream<'static, Result<bytes::Bytes, reqwest::Error>>"
        });
        let mixed_v3 = serde_json::from_value::<Bindings>(value).expect("deserialize mixed v3");
        assert!(mixed_v3.validate().is_err());
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
