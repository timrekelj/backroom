use std::time::Duration;

use chrono::{DateTime, Utc};
use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};

use crate::AppState;
use crate::auth::{
    DUMMY_PASSWORD_HASH, authenticate, create_session, hash_password, internal, peer_key,
    verify_password,
};
use crate::convert::{id_from_db, id_to_db, snowflake_datetime, user_to_proto};
use crate::proto::thehideout::v1::*;

const PASSWORD_LEN: std::ops::RangeInclusive<usize> = 8..=128;
const USERNAME_LEN: std::ops::RangeInclusive<usize> = 2..=32;

/// Hard cap on login attempts per peer IP within [`LOGIN_WINDOW`].
pub const LOGIN_IP_MAX: usize = 30;
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Per-username login attempts within [`LOGIN_WINDOW`] after which each further
/// attempt is delayed instead of rejected, so an attacker cannot lock out a
/// victim's account.
pub const LOGIN_USER_SOFT: usize = 5;

/// Sign-ups allowed per peer IP within [`SIGNUP_WINDOW`].
pub const SIGNUP_IP_MAX: usize = 20;
const SIGNUP_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Bounds a username used inside a rate-limit key so keys stay short.
fn key_username(username: &str) -> String {
    username.chars().take(64).collect()
}

pub struct AuthSvc {
    state: AppState,
}

impl AuthSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Trims and lowercases a username so lookups are case-insensitive.
fn normalize_username(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// Matches `^[a-z0-9_.]{2,32}$` on an already normalized username.
fn valid_username(username: &str) -> bool {
    USERNAME_LEN.contains(&username.len())
        && username
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

fn invalid_credentials() -> ConnectError {
    ConnectError::unauthenticated("invalid username or password")
}

impl AuthService for AuthSvc {
    async fn sign_up(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, SignUpRequest>,
    ) -> ServiceResult<SignUpResponse> {
        let username = normalize_username(req.username);
        // Throttle before doing any expensive work (argon2 hashing, writes).
        self.state
            .limiter
            .check(&format!("signup:ip:{}", peer_key(&ctx)), SIGNUP_IP_MAX, SIGNUP_WINDOW)?;
        if !valid_username(&username) {
            return Err(ConnectError::invalid_argument(
                "username must be 2-32 characters of a-z, 0-9, '_' or '.'",
            ));
        }
        if !PASSWORD_LEN.contains(&req.password.chars().count()) {
            return Err(ConnectError::invalid_argument(
                "password must be 8-128 characters",
            ));
        }

        let password_hash = hash_password(req.password.to_owned()).await?;
        let id = self.state.snowflake.next_id();
        let db_id = id_to_db(id);
        let created_at = snowflake_datetime(id);

        // Write the user row before claiming the username: a failure here leaves only an unreachable orphan, never a claimed name without an account.
        self.state
            .session
            .execute_unpaged(
                "INSERT INTO users (id, username, password_hash, created_at) VALUES (?, ?, ?, ?)",
                (db_id, &username, &password_hash, created_at),
            )
            .await
            .map_err(internal)?;

        // An LWT returns `[applied]` plus the existing row's columns when not applied.
        let (claimed, _, _) = self
            .state
            .session
            .execute_unpaged(
                "INSERT INTO users_by_username (username, user_id) VALUES (?, ?) IF NOT EXISTS",
                (&username, db_id),
            )
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .first_row::<(bool, Option<String>, Option<i64>)>()
            .map_err(internal)?;
        if !claimed {
            if let Err(err) = self
                .state
                .session
                .execute_unpaged("DELETE FROM users WHERE id = ?", (db_id,))
                .await
            {
                tracing::warn!(user_id = id, error = %err, "failed to delete orphaned user row");
            }
            // Generic error on purpose: a distinct "username taken" code would let an
            // unauthenticated caller probe which usernames have accounts, a signal the
            // login path deliberately hides. The sign-up rate limit bounds the residual
            // success/failure oracle.
            return Err(ConnectError::failed_precondition("could not create the account"));
        }

        let token = create_session(&self.state, db_id).await?;

        tracing::info!(user_id = id, "user signed up");
        Response::ok(SignUpResponse {
            token,
            user: user_to_proto(id, &username, created_at, None).into(),
            ..Default::default()
        })
    }

    async fn login(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, LoginRequest>,
    ) -> ServiceResult<LoginResponse> {
        let username = normalize_username(req.username);
        // Hard per-IP cap on floods. Per-username attempts are delayed past a soft
        // threshold, never rejected, so an attacker cannot lock out a victim's
        // account; the growing delay alone throttles distributed brute force.
        self.state
            .limiter
            .check(&format!("login:ip:{}", peer_key(&ctx)), LOGIN_IP_MAX, LOGIN_WINDOW)?;
        let attempts = self
            .state
            .limiter
            .record(&format!("login:user:{}", key_username(&username)), LOGIN_WINDOW);
        if attempts > LOGIN_USER_SOFT {
            tokio::time::sleep(Duration::from_secs((attempts - LOGIN_USER_SOFT).min(10) as u64)).await;
        }
        let found = self.find_user_by_username(&username).await?;

        let Some((db_id, hash, created_at, avatar)) = found else {
            // Burn the same CPU time as a real verification.
            verify_password(req.password.to_owned(), DUMMY_PASSWORD_HASH.to_owned()).await?;
            return Err(invalid_credentials());
        };
        if !verify_password(req.password.to_owned(), hash).await? {
            return Err(invalid_credentials());
        }

        let token = create_session(&self.state, db_id).await?;
        let id = id_from_db(db_id);
        tracing::info!(user_id = id, "user logged in");
        Response::ok(LoginResponse {
            token,
            user: user_to_proto(id, &username, created_at, avatar).into(),
            ..Default::default()
        })
    }

    async fn get_me(
        &self,
        ctx: RequestContext,
        _req: ServiceRequest<'_, GetMeRequest>,
    ) -> ServiceResult<GetMeResponse> {
        let id = authenticate(&self.state, &ctx).await?;
        let (username, created_at, avatar) = self
            .state
            .session
            .execute_unpaged("SELECT username, created_at, avatar FROM users WHERE id = ?", (id_to_db(id),))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(String, DateTime<Utc>, Option<String>)>()
            .map_err(internal)?
            .ok_or_else(|| ConnectError::unauthenticated("user no longer exists"))?;
        Response::ok(GetMeResponse {
            user: user_to_proto(id, &username, created_at, avatar).into(),
            ..Default::default()
        })
    }
}

impl AuthSvc {
    /// Looks up `(user id, password hash, created_at)` by normalized username.
    async fn find_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<(i64, String, DateTime<Utc>, Option<String>)>, ConnectError> {
        let Some((db_id,)) = self
            .state
            .session
            .execute_unpaged("SELECT user_id FROM users_by_username WHERE username = ?", (username,))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(i64,)>()
            .map_err(internal)?
        else {
            return Ok(None);
        };
        let row = self
            .state
            .session
            .execute_unpaged("SELECT password_hash, created_at, avatar FROM users WHERE id = ?", (db_id,))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(String, DateTime<Utc>, Option<String>)>()
            .map_err(internal)?;
        Ok(row.map(|(hash, created_at, avatar)| (db_id, hash, created_at, avatar)))
    }
}
