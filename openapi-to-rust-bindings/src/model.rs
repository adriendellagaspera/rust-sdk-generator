use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fmt;

/// Error returned when structured backend metadata or a Bindings sidecar cannot be normalized.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    message: String,
}

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Immutable normalized Rust binding sidecar independent from the generator runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bindings {
    value: Value,
}

impl Bindings {
    /// Validate and normalize a backend-neutral Bindings v2/v3 JSON value.
    pub fn from_value(mut value: Value) -> Result<Self, Error> {
        validate_bindings(&value)?;
        if let Some(operations) = value.get_mut("operations").and_then(Value::as_object_mut) {
            for operation in operations.values_mut() {
                if let Some(operation) = operation.as_object_mut() {
                    operation.entry("stream").or_insert(Value::Null);
                }
            }
        }
        Ok(Self { value })
    }

    /// Borrow the canonical JSON representation.
    pub fn as_value(&self) -> &Value {
        &self.value
    }

    /// Return an owned canonical JSON representation.
    pub fn to_value(&self) -> Value {
        self.value.clone()
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(format!("invalid Bindings: {}", message.into()))
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>, Error> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("{context} must be an object")))
}

fn array<'a>(value: &'a Value, context: &str) -> Result<&'a [Value], Error> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| invalid(format!("{context} must be an array")))
}

fn string<'a>(value: &'a Value, context: &str) -> Result<&'a str, Error> {
    value
        .as_str()
        .ok_or_else(|| invalid(format!("{context} must be a string")))
}

fn nonempty_string<'a>(value: &'a Value, context: &str) -> Result<&'a str, Error> {
    let value = string(value, context)?;
    if value.is_empty() {
        Err(invalid(format!("{context} must not be empty")))
    } else {
        Ok(value)
    }
}

fn nullable_string(value: &Value, context: &str) -> Result<(), Error> {
    if value.is_null() || value.is_string() {
        Ok(())
    } else {
        Err(invalid(format!("{context} must be a string or null")))
    }
}

fn boolean(value: &Value, context: &str) -> Result<(), Error> {
    if value.is_boolean() {
        Ok(())
    } else {
        Err(invalid(format!("{context} must be a boolean")))
    }
}

fn exact_keys(
    value: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
    context: &str,
) -> Result<(), Error> {
    for key in required {
        if !value.contains_key(*key) {
            return Err(invalid(format!("{context} is missing {key}")));
        }
    }
    for key in value.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(invalid(format!("{context} has unexpected property {key}")));
        }
    }
    Ok(())
}

fn validate_name_type(value: &Value, context: &str) -> Result<(), Error> {
    let value = object(value, context)?;
    exact_keys(value, &["name", "type"], &[], context)?;
    string(&value["name"], &format!("{context}.name"))?;
    string(&value["type"], &format!("{context}.type"))?;
    Ok(())
}

fn validate_field(value: &Value, context: &str, version: u64) -> Result<(), Error> {
    let value = object(value, context)?;
    if version == 3 {
        exact_keys(value, &["name", "wire_name", "type"], &[], context)?;
    } else {
        exact_keys(value, &["name", "type"], &["wire_name"], context)?;
    }
    string(&value["name"], &format!("{context}.name"))?;
    string(&value["type"], &format!("{context}.type"))?;
    if let Some(wire_name) = value.get("wire_name") {
        nullable_string(wire_name, &format!("{context}.wire_name"))?;
    }
    Ok(())
}

fn validate_stream(value: &Value, context: &str) -> Result<(), Error> {
    let stream = object(value, context)?;
    exact_keys(
        stream,
        &["item_type", "error_type", "lifetime"],
        &[],
        context,
    )?;
    for key in ["item_type", "error_type", "lifetime"] {
        nonempty_string(&stream[key], &format!("{context}.{key}"))?;
    }
    Ok(())
}

fn validate_representation(value: &Value, context: &str) -> Result<bool, Error> {
    let representation = object(value, context)?;
    let kind = representation
        .get("kind")
        .ok_or_else(|| invalid(format!("{context} is missing kind")))
        .and_then(|value| string(value, &format!("{context}.kind")))?;
    match kind {
        "json" => {
            exact_keys(
                representation,
                &["kind", "schema_name", "media_type"],
                &[],
                context,
            )?;
            nonempty_string(
                &representation["schema_name"],
                &format!("{context}.schema_name"),
            )?;
            nonempty_string(
                &representation["media_type"],
                &format!("{context}.media_type"),
            )?;
            Ok(false)
        }
        "text" | "event_stream" => {
            exact_keys(representation, &["kind", "media_type"], &[], context)?;
            nonempty_string(
                &representation["media_type"],
                &format!("{context}.media_type"),
            )?;
            Ok(kind == "event_stream")
        }
        "binary_buffered" | "binary_stream" => {
            exact_keys(
                representation,
                &["kind", "media_type", "wildcard"],
                &[],
                context,
            )?;
            nonempty_string(
                &representation["media_type"],
                &format!("{context}.media_type"),
            )?;
            boolean(&representation["wildcard"], &format!("{context}.wildcard"))?;
            Ok(kind == "binary_stream")
        }
        "empty" => {
            exact_keys(representation, &["kind"], &[], context)?;
            Ok(false)
        }
        other => Err(invalid(format!(
            "{context}.kind has unsupported value {other:?}"
        ))),
    }
}

fn representation_identity(value: &Value) -> Result<String, Error> {
    let representation = object(value, "representation")?;
    let kind = string(&representation["kind"], "representation.kind")?;
    let identity = match kind {
        "json" => format!(
            "json\u{0}{}\u{0}{}",
            string(&representation["schema_name"], "representation.schema_name")?,
            string(&representation["media_type"], "representation.media_type")?
        ),
        "text" | "event_stream" => format!(
            "{kind}\u{0}{}",
            string(&representation["media_type"], "representation.media_type")?
        ),
        "binary_buffered" | "binary_stream" => format!(
            "{kind}\u{0}{}\u{0}{}",
            string(&representation["media_type"], "representation.media_type")?,
            representation["wildcard"]
        ),
        "empty" => "empty".to_owned(),
        _ => return Err(invalid("representation kind is unsupported")),
    };
    Ok(identity)
}

fn validate_discriminator(value: &Value, context: &str) -> Result<(), Error> {
    let discriminator = object(value, context)?;
    exact_keys(
        discriminator,
        &[
            "wire_name",
            "rust_access_path",
            "rust_value_type",
            "value",
            "field_required",
            "field_nullable",
            "field_tri_state",
        ],
        &[],
        context,
    )?;
    nonempty_string(&discriminator["wire_name"], &format!("{context}.wire_name"))?;
    nonempty_string(
        &discriminator["rust_value_type"],
        &format!("{context}.rust_value_type"),
    )?;

    let access_path = array(
        &discriminator["rust_access_path"],
        &format!("{context}.rust_access_path"),
    )?;
    if access_path.is_empty() {
        return Err(invalid(format!(
            "{context}.rust_access_path must not be empty"
        )));
    }
    for (index, segment) in access_path.iter().enumerate() {
        nonempty_string(segment, &format!("{context}.rust_access_path[{index}]"))?;
    }

    let discriminator_value = &discriminator["value"];
    if !(discriminator_value.is_boolean()
        || discriminator_value.is_string()
        || discriminator_value.as_i64().is_some())
    {
        return Err(invalid(format!(
            "{context}.value must be a boolean, integer, or string"
        )));
    }
    for key in ["field_required", "field_nullable", "field_tri_state"] {
        boolean(&discriminator[key], &format!("{context}.{key}"))?;
    }
    Ok(())
}

fn validate_stream_abi(value: &Value, context: &str) -> Result<(), Error> {
    let abi = object(value, context)?;
    exact_keys(
        abi,
        &[
            "alias",
            "item_type",
            "error_type",
            "lifetime",
            "native_type",
            "wasm_type",
        ],
        &[],
        context,
    )?;
    for key in [
        "alias",
        "item_type",
        "error_type",
        "lifetime",
        "native_type",
        "wasm_type",
    ] {
        nonempty_string(&abi[key], &format!("{context}.{key}"))?;
    }
    Ok(())
}

fn validate_metadata(
    value: &Value,
    context: &str,
    operation_stream: Option<&Value>,
) -> Result<String, Error> {
    let metadata = object(value, context)?;
    exact_keys(
        metadata,
        &[
            "kind",
            "source_operation",
            "emitted_operation_id",
            "representation",
            "success_statuses",
            "request_discriminators",
            "stream_abi",
        ],
        &[],
        context,
    )?;

    let kind = string(&metadata["kind"], &format!("{context}.kind"))?;
    if !matches!(kind, "call_shape" | "multipart_filenames") {
        return Err(invalid(format!(
            "{context}.kind has unsupported value {kind:?}"
        )));
    }

    let source_context = format!("{context}.source_operation");
    let source = object(&metadata["source_operation"], &source_context)?;
    exact_keys(
        source,
        &["operation_id", "method", "path"],
        &[],
        &source_context,
    )?;
    let operation_id = nonempty_string(
        &source["operation_id"],
        &format!("{source_context}.operation_id"),
    )?;
    let method = nonempty_string(&source["method"], &format!("{source_context}.method"))?;
    let path = nonempty_string(&source["path"], &format!("{source_context}.path"))?;
    nonempty_string(
        &metadata["emitted_operation_id"],
        &format!("{context}.emitted_operation_id"),
    )?;

    let streaming = validate_representation(
        &metadata["representation"],
        &format!("{context}.representation"),
    )?;

    let statuses = array(
        &metadata["success_statuses"],
        &format!("{context}.success_statuses"),
    )?;
    let mut seen_statuses = BTreeSet::new();
    for (index, status) in statuses.iter().enumerate() {
        let status = string(status, &format!("{context}.success_statuses[{index}]"))?;
        if !seen_statuses.insert(status) {
            return Err(invalid(format!(
                "{context}.success_statuses must contain unique values"
            )));
        }
    }

    for (index, discriminator) in array(
        &metadata["request_discriminators"],
        &format!("{context}.request_discriminators"),
    )?
    .iter()
    .enumerate()
    {
        validate_discriminator(
            discriminator,
            &format!("{context}.request_discriminators[{index}]"),
        )?;
    }

    let stream_abi = &metadata["stream_abi"];
    if stream_abi.is_null() != !streaming {
        return Err(invalid(format!(
            "{context}.stream_abi presence must match response representation"
        )));
    }
    if streaming {
        validate_stream_abi(stream_abi, &format!("{context}.stream_abi"))?;
    }

    let common_stream_present = operation_stream.is_some_and(|stream| !stream.is_null());
    if common_stream_present != streaming {
        return Err(invalid(format!(
            "{context} stream ABI disagrees with operation.stream"
        )));
    }
    if let Some(stream) = operation_stream.filter(|stream| !stream.is_null()) {
        let stream = object(stream, "operation.stream")?;
        let abi = object(stream_abi, &format!("{context}.stream_abi"))?;
        for key in ["item_type", "error_type", "lifetime"] {
            if stream[key] != abi[key] {
                return Err(invalid(format!(
                    "{context}.stream_abi.{key} disagrees with operation.stream.{key}"
                )));
            }
        }
    }

    Ok(format!(
        "{kind}\u{0}{operation_id}\u{0}{method}\u{0}{path}\u{0}{}",
        representation_identity(&metadata["representation"])?
    ))
}

fn validate_bindings(value: &Value) -> Result<(), Error> {
    let root = object(value, "root")?;
    exact_keys(
        root,
        &[
            "schema_version",
            "structs",
            "enums",
            "aliases",
            "operations",
            "symbol_paths",
            "binding",
        ],
        &[],
        "root",
    )?;
    let version = root["schema_version"]
        .as_u64()
        .ok_or_else(|| invalid("schema_version must be an integer"))?;
    if !matches!(version, 2 | 3) {
        return Err(invalid(format!(
            "unsupported schema_version {version}; expected 2 or 3"
        )));
    }

    let structs = object(&root["structs"], "structs")?;
    for (name, fields) in structs {
        for (index, field) in array(fields, &format!("structs.{name}"))?
            .iter()
            .enumerate()
        {
            validate_field(field, &format!("structs.{name}[{index}]"), version)?;
        }
    }

    let enums = object(&root["enums"], "enums")?;
    for (name, variants) in enums {
        for (index, variant) in array(variants, &format!("enums.{name}"))?
            .iter()
            .enumerate()
        {
            let context = format!("enums.{name}[{index}]");
            let variant = object(variant, &context)?;
            exact_keys(variant, &["name", "payload", "wire_name"], &[], &context)?;
            string(&variant["name"], &format!("{context}.name"))?;
            nullable_string(&variant["payload"], &format!("{context}.payload"))?;
            nullable_string(&variant["wire_name"], &format!("{context}.wire_name"))?;
        }
    }

    let aliases = object(&root["aliases"], "aliases")?;
    for (name, alias) in aliases {
        string(alias, &format!("aliases.{name}"))?;
    }

    let operations = object(&root["operations"], "operations")?;
    let mut canonical_identities = BTreeSet::new();
    for (name, operation) in operations {
        let context = format!("operations.{name}");
        let operation = object(operation, &context)?;
        if version == 3 {
            exact_keys(
                operation,
                &[
                    "name",
                    "parameters",
                    "return_type",
                    "success_type",
                    "metadata",
                ],
                &["stream"],
                &context,
            )?;
        } else {
            exact_keys(
                operation,
                &["name", "parameters", "return_type", "success_type"],
                &["stream"],
                &context,
            )?;
        }
        let operation_name = string(&operation["name"], &format!("{context}.name"))?;
        if version == 3 && operation_name != name {
            return Err(invalid(format!(
                "{context}.name must equal its operation map key"
            )));
        }
        string(&operation["return_type"], &format!("{context}.return_type"))?;
        string(
            &operation["success_type"],
            &format!("{context}.success_type"),
        )?;
        for (index, parameter) in array(&operation["parameters"], &format!("{context}.parameters"))?
            .iter()
            .enumerate()
        {
            validate_name_type(parameter, &format!("{context}.parameters[{index}]"))?;
        }
        if let Some(stream) = operation.get("stream")
            && !stream.is_null()
        {
            validate_stream(stream, &format!("{context}.stream"))?;
        }

        if version == 3 {
            let identity = validate_metadata(
                &operation["metadata"],
                &format!("{context}.metadata"),
                operation.get("stream"),
            )?;
            if !canonical_identities.insert(identity) {
                return Err(invalid(format!(
                    "{context}.metadata duplicates canonical source-operation/representation identity"
                )));
            }
        }
    }

    let symbol_paths = object(&root["symbol_paths"], "symbol_paths")?;
    for (name, path) in symbol_paths {
        string(path, &format!("symbol_paths.{name}"))?;
    }

    let binding = object(&root["binding"], "binding")?;
    exact_keys(binding, &["client", "type_preludes"], &[], "binding")?;
    let client = object(&binding["client"], "binding.client")?;
    exact_keys(
        client,
        &[
            "type_path",
            "constructor",
            "api_key_builder",
            "base_url_builder",
        ],
        &[],
        "binding.client",
    )?;
    for key in [
        "type_path",
        "constructor",
        "api_key_builder",
        "base_url_builder",
    ] {
        string(&client[key], &format!("binding.client.{key}"))?;
    }

    let preludes = array(&binding["type_preludes"], "binding.type_preludes")?;
    let mut seen = BTreeSet::new();
    for (index, prelude) in preludes.iter().enumerate() {
        let prelude = string(prelude, &format!("binding.type_preludes[{index}]"))?;
        if !seen.insert(prelude) {
            return Err(invalid("binding.type_preludes must contain unique values"));
        }
    }

    Ok(())
}
