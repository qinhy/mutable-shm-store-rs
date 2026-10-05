use thiserror::Error;

pub type Result<T> = std::result::Result<T, MStoreError>;

#[derive(Debug, Error)]
pub enum MStoreError {
    #[error("{0}")]
    Authentication(String),
    #[error("{0}")]
    PermissionDenied(String),
    #[error("{0}")]
    ObjectNotFound(String),
    #[error("{0}")]
    TokenExpired(String),
    #[error("{0}")]
    TokenRevoked(String),
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{0}")]
    Protocol(String),
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Array(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl MStoreError {
    pub fn protocol_code(&self) -> &'static str {
        match self {
            Self::Authentication(_) => "authentication_error",
            Self::PermissionDenied(_) => "permission_denied",
            Self::ObjectNotFound(_) => "object_not_found",
            Self::TokenExpired(_) => "token_expired",
            Self::TokenRevoked(_) => "token_revoked",
            Self::InvalidRequest(_) => "invalid_request",
            Self::Protocol(_) => "protocol_error",
            Self::Internal(_) | Self::Unsupported(_) | Self::Array(_) | Self::Io(_) | Self::Json(_) => {
                "internal_error"
            }
        }
    }

    pub fn from_remote(code: &str, message: String) -> Self {
        match code {
            "authentication_error" => Self::Authentication(message),
            "permission_denied" => Self::PermissionDenied(message),
            "object_not_found" => Self::ObjectNotFound(message),
            "token_expired" => Self::TokenExpired(message),
            "token_revoked" => Self::TokenRevoked(message),
            "invalid_request" => Self::InvalidRequest(message),
            "protocol_error" => Self::Protocol(message),
            "internal_error" => Self::Internal(message),
            _ => Self::Protocol(format!("remote {code}: {message}")),
        }
    }
}
