//! Minimal reviewed consumer runtime for the v1 upstream client ABI.
//! This is an integration example, not a production error taxonomy or auth policy.
use crate::generated::client::ApiOpError;

#[derive(Debug)]
pub enum SdkError {
    Api {
        status: u16,
        body: String,
        parse_error: Option<String>,
    },
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

// The backend-neutral facade's current default Runtime requires these exports.
// Review and replace them alongside the consumer's production error policy.
pub type ApiError = SdkError;
pub type TransportError = SdkError;
pub type TransportErrorKind = SdkError;
