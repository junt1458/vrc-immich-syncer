//! Error types for the Immich API client.

/// Errors that can occur while talking to an Immich server.
#[derive(Debug, thiserror::Error)]
pub enum ImmichError {
    /// Low-level transport error (connection refused, TLS error, timeout, …).
    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// The server responded with a non-2xx status. If the body could be
    /// parsed as an Immich error payload, its message is included.
    #[error("Immich API error ({status}): {message}")]
    Api {
        status: reqwest::StatusCode,
        message: String,
    },

    /// The response body could not be decoded as the expected JSON shape.
    #[error("failed to decode JSON response: {0}")]
    Decode(serde_json::Error),

    /// The configured API key was rejected (HTTP 401).
    #[error("authentication failed: invalid or expired API key")]
    Unauthorized,

    /// A request body could not be constructed (e.g. bad multipart part).
    #[error("failed to build request: {0}")]
    InvalidRequest(String),
}

/// Body shape Immich commonly returns on error responses, e.g.:
/// `{"message": "...", "error": "...", "statusCode": 400}`
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ImmichErrorBody {
    #[serde(default)]
    pub message: Option<ImmichErrorMessage>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Immich's `message` field is sometimes a single string and sometimes an
/// array of validation error strings, depending on the endpoint.
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub(crate) enum ImmichErrorMessage {
    Single(String),
    Multiple(Vec<String>),
}

impl std::fmt::Display for ImmichErrorMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImmichErrorMessage::Single(s) => write!(f, "{s}"),
            ImmichErrorMessage::Multiple(items) => write!(f, "{}", items.join("; ")),
        }
    }
}

impl ImmichErrorBody {
    pub(crate) fn into_message(self) -> Option<String> {
        match (self.message, self.error) {
            (Some(msg), _) => Some(msg.to_string()),
            (None, Some(err)) => Some(err),
            (None, None) => None,
        }
    }
}
