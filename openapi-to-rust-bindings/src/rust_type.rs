use crate::Error;
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;

fn failure(detail: impl std::fmt::Display) -> Error {
    Error::new(format!("extract.rust_type_unrenderable: {detail}"))
}

fn push_word(output: &mut String, word: &str) {
    let needs_space = output
        .chars()
        .last()
        .is_some_and(|last| last.is_ascii_alphanumeric() || last == '_' || last == '!');
    if needs_space {
        output.push(' ');
    }
    output.push_str(word);
}

fn render_stream(stream: TokenStream) -> String {
    let tokens: Vec<_> = stream.into_iter().collect();
    let mut output = String::new();
    let mut index = 0;
    while index < tokens.len() {
        match &tokens[index] {
            TokenTree::Ident(ident) => push_word(&mut output, &ident.to_string()),
            TokenTree::Literal(literal) => push_word(&mut output, &literal.to_string()),
            TokenTree::Group(group) => {
                let (open, close) = match group.delimiter() {
                    Delimiter::Parenthesis => ('(', ')'),
                    Delimiter::Brace => ('{', '}'),
                    Delimiter::Bracket => ('[', ']'),
                    Delimiter::None => ('\0', '\0'),
                };
                if open != '\0' {
                    output.push(open);
                }
                output.push_str(&render_stream(group.stream()));
                if close != '\0' {
                    output.push(close);
                }
            }
            TokenTree::Punct(punct) => {
                let ch = punct.as_char();
                let next_punct = tokens.get(index + 1).and_then(|token| match token {
                    TokenTree::Punct(value) => Some(value.as_char()),
                    _ => None,
                });
                match (ch, next_punct) {
                    (':', Some(':')) => {
                        while output.ends_with(' ') {
                            output.pop();
                        }
                        output.push_str("::");
                        index += 1;
                    }
                    ('-', Some('>')) => {
                        while output.ends_with(' ') {
                            output.pop();
                        }
                        output.push_str(" -> ");
                        index += 1;
                    }
                    ('=', Some('>')) => {
                        while output.ends_with(' ') {
                            output.pop();
                        }
                        output.push_str(" => ");
                        index += 1;
                    }
                    ('<', _) => {
                        while output.ends_with(' ') {
                            output.pop();
                        }
                        output.push('<');
                    }
                    ('>', _) => {
                        while output.ends_with(' ') {
                            output.pop();
                        }
                        output.push('>');
                    }
                    (',', _) => {
                        while output.ends_with(' ') {
                            output.pop();
                        }
                        output.push_str(", ");
                    }
                    ('&' | '*' | '\'' | '?' | '!', _) => {
                        output.push(ch);
                    }
                    ('+' | '=' | '|', _) => {
                        while output.ends_with(' ') {
                            output.pop();
                        }
                        output.push(' ');
                        output.push(ch);
                        output.push(' ');
                    }
                    (':', _) => {
                        while output.ends_with(' ') {
                            output.pop();
                        }
                        output.push_str(": ");
                    }
                    (';', _) => {
                        while output.ends_with(' ') {
                            output.pop();
                        }
                        output.push_str("; ");
                    }
                    _ => output.push(ch),
                }
            }
        }
        index += 1;
    }
    output.trim().to_owned()
}

pub(crate) fn canonical_rust_type(source: &str) -> Result<String, Error> {
    let parsed: syn::Type =
        syn::parse_str(source).map_err(|error| failure(format!("{source:?}: {error}")))?;
    Ok(render_stream(parsed.to_token_stream()))
}

#[cfg(test)]
mod tests {
    use super::canonical_rust_type;

    #[test]
    fn renders_backend_manifest_style_types() {
        for (source, expected) in [
            ("Vec < Model >", "Vec<Model>"),
            ("Option < bool >", "Option<bool>"),
            ("impl AsRef < str >", "impl AsRef<str>"),
            ("Result < Item , Error >", "Result<Item, Error>"),
            ("& [(& str , & str)]", "&[(&str, &str)]"),
            (
                "futures_util :: stream :: BoxStream < 'static , Result < bytes :: Bytes , reqwest :: Error > >",
                "futures_util::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>",
            ),
        ] {
            assert_eq!(canonical_rust_type(source).unwrap(), expected);
        }
    }
}
