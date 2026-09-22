use std::collections::BTreeMap;

use crate::error::{GenerationError, Result};

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "gen", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "union",
];

fn is_keyword(name: &str) -> bool {
    KEYWORDS.contains(&name)
}

fn is_identifier(name: &str) -> bool {
    if name.is_empty() || !name.is_ascii() {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(crate) fn field_identifier(name: &str) -> Result<String> {
    let name = name.strip_prefix("r#").unwrap_or(name);
    if matches!(name, "self" | "Self" | "super" | "crate" | "_") {
        return Err(GenerationError::new(
            "symbol.invalid_field",
            format!("Rust cannot represent field identifier {name:?}; explicit mapping required"),
        ));
    }
    Ok(if is_keyword(name) {
        format!("r#{name}")
    } else {
        name.into()
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Symbol {
    pub name: String,
    pub namespace: String,
    pub source: String,
    pub module: String,
}

#[derive(Debug, Default)]
pub(crate) struct SymbolProvider {
    symbols: BTreeMap<(String, String), Symbol>,
}

impl SymbolProvider {
    pub fn claim(
        &mut self,
        name: &str,
        namespace: &str,
        source: &str,
        module: &str,
    ) -> Result<Symbol> {
        if !is_identifier(name) || name == "_" || is_keyword(name) {
            return Err(GenerationError::new(
                "symbol.invalid",
                format!(
                    "invalid/reserved Rust symbol {name:?} at {source}; choose an explicit semantic name"
                ),
            ));
        }
        let key = (namespace.to_owned(), name.to_owned());
        if let Some(previous) = self.symbols.get(&key) {
            return Err(GenerationError::new(
                "symbol.collision",
                format!(
                    "Rust symbol collision {namespace}::{name}: {} vs {source}",
                    previous.source
                ),
            ));
        }
        let symbol = Symbol {
            name: name.into(),
            namespace: namespace.into(),
            source: source.into(),
            module: module.into(),
        };
        self.symbols.insert(key, symbol.clone());
        Ok(symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_protocol_owned_keywords() {
        assert_eq!(field_identifier("type").expect("escapable"), "r#type");
        assert!(field_identifier("self").is_err());
    }

    #[test]
    fn collisions_fail_closed() {
        let mut provider = SymbolProvider::default();
        provider
            .claim("Thing", "sdk", "first", "")
            .expect("first claim");
        let error = provider
            .claim("Thing", "sdk", "second", "")
            .expect_err("collision");
        assert_eq!(error.diagnostic.code, "symbol.collision");
    }
}
