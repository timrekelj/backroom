//! The error type of every API call.

use std::fmt;

use connectrpc::{ConnectError, ErrorCode};

/// Why an API call failed, mapped from the Connect error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// Missing, invalid or expired session token, or wrong username/password.
    Unauthenticated,
    /// The request was rejected as malformed (empty or over-long message, bad username, ...).
    InvalidArgument(String),
    /// The guild, channel, user or invite does not exist (or is not visible to the caller).
    NotFound,
    /// The caller is not allowed to do this (e.g. creating a channel in a guild they don't own).
    PermissionDenied,
    /// The change was based on a stale version (layout or role order); refetch and retry.
    Conflict,
    /// The server does not implement this call.
    Unimplemented,
    /// Too many attempts; try again later.
    RateLimited,
    /// The resource already exists. Sign-up reports a taken username this way.
    AlreadyExists,
    /// The server could not be reached or the connection broke.
    Network(String),
    /// Anything else, with the server's message.
    Other(String),
}

impl ApiError {
    /// Whether retrying the same call later might succeed.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Network(_) | Self::RateLimited)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthenticated => f.write_str("not signed in or invalid credentials"),
            Self::InvalidArgument(msg) => write!(f, "invalid request: {msg}"),
            Self::NotFound => f.write_str("not found"),
            Self::PermissionDenied => f.write_str("permission denied"),
            Self::Conflict => f.write_str("changed by someone else, refresh and try again"),
            Self::Unimplemented => f.write_str("not supported by this server"),
            Self::RateLimited => f.write_str("too many attempts, try again later"),
            Self::AlreadyExists => f.write_str("already exists"),
            Self::Network(msg) => write!(f, "network error: {msg}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<ConnectError> for ApiError {
    fn from(err: ConnectError) -> Self {
        let msg = err.message.clone().unwrap_or_default();
        match err.code {
            ErrorCode::Unauthenticated => Self::Unauthenticated,
            ErrorCode::InvalidArgument | ErrorCode::OutOfRange => Self::InvalidArgument(msg),
            ErrorCode::NotFound => Self::NotFound,
            ErrorCode::PermissionDenied => Self::PermissionDenied,
            ErrorCode::Aborted => Self::Conflict,
            ErrorCode::Unimplemented => Self::Unimplemented,
            ErrorCode::ResourceExhausted => Self::RateLimited,
            // The server answers a taken username with FailedPrecondition.
            ErrorCode::AlreadyExists | ErrorCode::FailedPrecondition => Self::AlreadyExists,
            ErrorCode::Unavailable | ErrorCode::DeadlineExceeded => Self::Network(msg),
            _ => Self::Other(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(code: ErrorCode) -> ApiError {
        ConnectError::new(code, "boom").into()
    }

    #[test]
    fn maps_connect_codes() {
        assert_eq!(map(ErrorCode::Unauthenticated), ApiError::Unauthenticated);
        assert_eq!(map(ErrorCode::InvalidArgument), ApiError::InvalidArgument("boom".into()));
        assert_eq!(map(ErrorCode::NotFound), ApiError::NotFound);
        assert_eq!(map(ErrorCode::PermissionDenied), ApiError::PermissionDenied);
        assert_eq!(map(ErrorCode::ResourceExhausted), ApiError::RateLimited);
        assert_eq!(map(ErrorCode::AlreadyExists), ApiError::AlreadyExists);
        assert_eq!(map(ErrorCode::FailedPrecondition), ApiError::AlreadyExists);
        assert_eq!(map(ErrorCode::Unavailable), ApiError::Network("boom".into()));
        assert_eq!(map(ErrorCode::Aborted), ApiError::Conflict);
        assert_eq!(map(ErrorCode::Unimplemented), ApiError::Unimplemented);
        assert_eq!(map(ErrorCode::Internal), ApiError::Other("boom".into()));
    }

    #[test]
    fn transient_errors() {
        assert!(ApiError::Network(String::new()).is_transient());
        assert!(ApiError::RateLimited.is_transient());
        assert!(!ApiError::NotFound.is_transient());
    }
}
