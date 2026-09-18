use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fmt;

/// Error returned when generated Rust or a Bindings sidecar cannot be normalized.
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
    /// Validate and normalize a Bindings v2 JSON value.
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

fn string(value: &Value, context: &str) -> Result<(), Error> {
    if value.is_string() {
        Ok(())
    } else {
        Err(invalid(format!("{context} must be a string")))
    }
}

fn nullable_string(value: &Value, context: &str) -> Result<(), Error> {
    if value.is_null() || value.is_string() {
        Ok(())
    } else {
        Err(invalid(format!("{context} must be a string or null")))
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
    string(&value["type"], &format!("{context}.type"))
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
    if root["schema_version"] != 2 {
        return Err(invalid("schema_version must be 2"));
    }

    let structs = object(&root["structs"], "structs")?;
    for (name, fields) in structs {
        for (index, field) in array(fields, &format!("structs.{name}"))?
            .iter()
            .enumerate()
        {
            validate_name_type(field, &format!("structs.{name}[{index}]"))?;
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
    for (name, operation) in operations {
        let context = format!("operations.{name}");
        let operation = object(operation, &context)?;
        exact_keys(
            operation,
            &["name", "parameters", "return_type", "success_type"],
            &["stream"],
            &context,
        )?;
        string(&operation["name"], &format!("{context}.name"))?;
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
            let stream_context = format!("{context}.stream");
            let stream = object(stream, &stream_context)?;
            exact_keys(
                stream,
                &["item_type", "error_type", "lifetime"],
                &[],
                &stream_context,
            )?;
            for key in ["item_type", "error_type", "lifetime"] {
                string(&stream[key], &format!("{stream_context}.{key}"))?;
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
        string(prelude, &format!("binding.type_preludes[{index}]"))?;
        let prelude = prelude.as_str().expect("validated string");
        if !seen.insert(prelude) {
            return Err(invalid("binding.type_preludes must contain unique values"));
        }
    }

    Ok(())
}
