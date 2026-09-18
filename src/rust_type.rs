use crate::error::{GenerationError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Opaque,
    Generic,
}

/// Structural view of a normalized Rust type spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Type {
    pub kind: TypeKind,
    pub spelling: String,
    pub constructor: Option<String>,
    pub arguments: Vec<Type>,
}

impl Type {
    pub fn unary(&self, constructor: &str) -> Option<&Type> {
        (self.kind == TypeKind::Generic
            && self.constructor.as_deref() == Some(constructor)
            && self.arguments.len() == 1)
            .then(|| &self.arguments[0])
    }
}

fn type_error(message: impl Into<String>) -> GenerationError {
    GenerationError::new("rust_type.invalid", message)
}

fn validate_type_spelling(value: &str) -> Result<()> {
    let mut angles = 0_i32;
    let mut parentheses = 0_i32;
    let mut brackets = 0_i32;
    let mut braces = 0_i32;
    let mut in_string = false;
    let mut escaped = false;

    for ch in value.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '<' => angles += 1,
            '>' => angles -= 1,
            '(' => parentheses += 1,
            ')' => parentheses -= 1,
            '[' => brackets += 1,
            ']' => brackets -= 1,
            '{' => braces += 1,
            '}' => braces -= 1,
            ';' if angles == 0 && parentheses == 0 && brackets == 0 && braces == 0 => {
                return Err(type_error("unexpected Rust statement boundary in type"));
            }
            _ => {}
        }
        if angles < 0 || parentheses < 0 || brackets < 0 || braces < 0 {
            return Err(type_error("unbalanced Rust type delimiters"));
        }
    }
    if in_string || angles != 0 || parentheses != 0 || brackets != 0 || braces != 0 {
        return Err(type_error("unbalanced Rust type delimiters"));
    }
    Ok(())
}

fn split_arguments(value: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut angles = 0_i32;
    let mut parentheses = 0_i32;
    let mut brackets = 0_i32;
    let mut braces = 0_i32;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in value.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '<' => angles += 1,
            '>' => angles -= 1,
            '(' => parentheses += 1,
            ')' => parentheses -= 1,
            '[' => brackets += 1,
            ']' => brackets -= 1,
            '{' => braces += 1,
            '}' => braces -= 1,
            ',' if angles == 0 && parentheses == 0 && brackets == 0 && braces == 0 => {
                let part = value[start..index].trim();
                if part.is_empty() {
                    return Err(type_error("empty Rust generic argument"));
                }
                parts.push(part);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
        if angles < 0 || parentheses < 0 || brackets < 0 || braces < 0 {
            return Err(type_error("unbalanced Rust type delimiters"));
        }
    }
    if in_string || angles != 0 || parentheses != 0 || brackets != 0 || braces != 0 {
        return Err(type_error("unbalanced Rust type delimiters"));
    }
    let final_part = value[start..].trim();
    if final_part.is_empty() {
        if !parts.is_empty() && value.trim_end().ends_with(',') {
            return Ok(parts);
        }
        return Err(type_error("empty Rust generic argument"));
    }
    parts.push(final_part);
    Ok(parts)
}

fn outer_generic(spelling: &str) -> Result<Option<(&str, &str)>> {
    let mut depth = 0_i32;
    let mut first = None;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in spelling.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            continue;
        }
        if ch == '<' {
            if depth == 0 {
                first = Some(index);
            }
            depth += 1;
        } else if ch == '>' {
            depth -= 1;
            if depth < 0 {
                return Err(type_error("unbalanced Rust generic delimiters"));
            }
            if depth == 0 && index + ch.len_utf8() != spelling.len() {
                return Ok(None);
            }
        }
    }
    if depth != 0 || in_string {
        return Err(type_error("unbalanced Rust generic delimiters"));
    }
    let Some(first) = first else {
        return Ok(None);
    };
    let constructor = spelling[..first].trim();
    if constructor.is_empty() || constructor.chars().any(char::is_whitespace) {
        return Ok(None);
    }
    Ok(Some((
        constructor,
        &spelling[first + 1..spelling.len() - 1],
    )))
}

pub fn parse_type(spelling: &str) -> Result<Type> {
    let spelling = spelling.trim();
    if spelling.is_empty() {
        return Err(type_error("empty Rust type"));
    }
    validate_type_spelling(spelling)?;
    let Some((constructor, arguments)) = outer_generic(spelling)? else {
        return Ok(Type {
            kind: TypeKind::Opaque,
            spelling: spelling.into(),
            constructor: None,
            arguments: Vec::new(),
        });
    };
    let arguments = split_arguments(arguments)?
        .into_iter()
        .map(parse_type)
        .collect::<Result<Vec<_>>>()?;
    Ok(Type {
        kind: TypeKind::Generic,
        spelling: spelling.into(),
        constructor: Some(constructor.into()),
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_generics_without_reformatting() {
        let parsed = parse_type("Option<Vec<Result<String, Error>>>").expect("valid type");
        assert_eq!(parsed.constructor.as_deref(), Some("Option"));
        let vector = parsed.unary("Option").expect("option");
        assert_eq!(vector.constructor.as_deref(), Some("Vec"));
        assert_eq!(parsed.spelling, "Option<Vec<Result<String, Error>>>");
    }

    #[test]
    fn accepts_trailing_comma_in_generic_arguments() {
        let parsed = parse_type(
            "std::collections::BTreeMap<\n    String,\n    Option<serde_json::Value>,\n>",
        )
        .expect("valid Rust generic with trailing comma");
        assert_eq!(
            parsed.constructor.as_deref(),
            Some("std::collections::BTreeMap")
        );
        assert_eq!(parsed.arguments.len(), 2);
        assert_eq!(parsed.arguments[0].spelling, "String");
        assert_eq!(
            parsed.arguments[1].spelling,
            "Option<serde_json::Value>"
        );
    }

    #[test]
    fn rejects_empty_internal_generic_arguments() {
        for spelling in ["Result<, Error>", "Result<String,, Error>"] {
            let error = parse_type(spelling).expect_err("must reject empty argument");
            assert_eq!(error.diagnostic.code, "rust_type.invalid");
        }
    }

    #[test]
    fn retains_non_generic_syntax_as_opaque() {
        let parsed = parse_type("[u8; 32]").expect("valid array");
        assert_eq!(parsed.kind, TypeKind::Opaque);
        assert_eq!(parsed.spelling, "[u8; 32]");
    }

    #[test]
    fn rejects_statement_suffix() {
        let error = parse_type("String; fn injected()").expect_err("must reject suffix");
        assert_eq!(error.diagnostic.code, "rust_type.invalid");
    }
}
