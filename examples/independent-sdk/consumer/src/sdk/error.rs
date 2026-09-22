//! Minimal, consumer-owned runtime adapter.
use crate::generated::client::ApiOpError;

#[derive(Debug)]
pub enum SdkError {
    Api { status: u16, body: String, parse_error: Option<String> },
    Transport(String),
}

impl<E: std::fmt::Debug> From<ApiOpError<E>> for SdkError {
    fn from(error: ApiOpError<E>) -> Self {
        match error {
            ApiOpError::Api(response) => Self::Api {
                status: response.status,
                body: response.body,
                parse_error: response.parse_error,
            },
            ApiOpError::Transport(error) => Self::Transport(error.to_string()),
        }
    }
}

impl From<reqwest::Error> for SdkError {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(error.to_string())
    }
}

// The generator's default module contract requires these exports; the
// independent proof only needs SdkError's two variants.
pub type ApiError = SdkError;
pub type TransportError = SdkError;
pub type TransportErrorKind = SdkError;
