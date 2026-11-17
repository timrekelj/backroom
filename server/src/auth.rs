//! Password hashing, session tokens and request authentication.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use connectrpc::{ConnectError, RequestContext};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::convert::id_from_db;

/// How long a session token stays valid.
pub const SESSION_TTL_SECS: i32 = 30 * 24 * 60 * 60;

/// A valid argon2id hash that never matches a real password; verified against
/// when a username is unknown so the response time looks the same.
pub const DUMMY_PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$BkC5oC+yWwX2FD0lJH6cOQ$g/hvc4UrZJsrTknSwxPBUY6ZFwTpPhSVD9nmLncBR7U";

/// Verifies the session token in `Authorization: Bearer <token>` and returns the user id.
pub async fn authenticate(state: &AppState, ctx: &RequestContext) -> Result<u64, ConnectError> {
    let token = ctx
        .header("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ConnectError::unauthenticated("missing or malformed authorization header"))?;

    let rows = state
        .session
        .execute_unpaged("SELECT user_id FROM sessions WHERE token_hash = ?", (hash_token(token),))
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?;
    let user_id = rows
        .maybe_first_row::<(i64,)>()
        .map_err(internal)?
        .map(|(id,)| id_from_db(id))
        .ok_or_else(|| ConnectError::unauthenticated("invalid or expired session token"))?;
    Ok(user_id)
}

/// Creates a session for `user_id` and returns the plaintext token (only its hash is stored).
pub async fn create_session(state: &AppState, user_id: i64) -> Result<String, ConnectError> {
    let token = generate_token();
    state
        .session
        .execute_unpaged(
            "INSERT INTO sessions (token_hash, user_id, created_at) VALUES (?, ?, toTimestamp(now())) USING TTL ?",
            (hash_token(&token), user_id, SESSION_TTL_SECS),
        )
        .await
        .map_err(internal)?;
    Ok(token)
}

/// 32 random bytes from the OS-seeded CSPRNG, base64url without padding.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Hex SHA-256 of a token, as stored in `sessions.token_hash`.
pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Hashes a password with argon2id (default params) off the async runtime.
pub async fn hash_password(password: String) -> Result<String, ConnectError> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(internal)?
    .map_err(internal)
}

/// Checks a password against a stored hash off the async runtime.
pub async fn verify_password(password: String, hash: String) -> Result<bool, ConnectError> {
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&hash).map_err(|e| e.to_string())?;
        Ok::<_, String>(Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
    })
    .await
    .map_err(internal)?
    .map_err(internal)
}

/// Logs the real cause and returns an opaque internal error.
pub fn internal(err: impl std::fmt::Display) -> ConnectError {
    tracing::error!(error = %err, "internal error");
    ConnectError::internal("internal error")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dummy_hash_is_valid_and_rejects_guesses() {
        assert!(!verify_password("password".into(), DUMMY_PASSWORD_HASH.into()).await.unwrap());
    }

    #[tokio::test]
    async fn hash_and_verify_round_trip() {
        let hash = hash_password("correct horse".into()).await.unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse".into(), hash.clone()).await.unwrap());
        assert!(!verify_password("wrong horse".into(), hash).await.unwrap());
    }

    #[test]
    fn tokens_are_unique_urlsafe_and_hash_is_hex() {
        let (a, b) = (generate_token(), generate_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert!(a.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        let h = hash_token(&a);
        assert_eq!(h.len(), 64);
        assert!(h.bytes().all(|c| c.is_ascii_hexdigit()));
    }
}
