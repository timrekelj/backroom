use chrono::{DateTime, Utc};
use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};

use crate::AppState;
use crate::auth::{
    DUMMY_PASSWORD_HASH, authenticate, create_session, hash_password, internal, verify_password,
};
use crate::convert::{id_from_db, id_to_db, snowflake_datetime, user_to_proto};
use crate::proto::backroom::v1::*;

const PASSWORD_LEN: std::ops::RangeInclusive<usize> = 8..=128;
const USERNAME_LEN: std::ops::RangeInclusive<usize> = 2..=32;

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
        _ctx: RequestContext,
        req: ServiceRequest<'_, SignUpRequest>,
    ) -> ServiceResult<SignUpResponse> {
        let username = normalize_username(req.username);
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
            return Err(ConnectError::already_exists("username is already taken"));
        }

        let token = create_session(&self.state, db_id).await?;

        tracing::info!(user_id = id, "user signed up");
        Response::ok(SignUpResponse {
            token,
            user: user_to_proto(id, &username, created_at).into(),
            ..Default::default()
        })
    }

    async fn login(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, LoginRequest>,
    ) -> ServiceResult<LoginResponse> {
        let username = normalize_username(req.username);
        let found = self.find_user_by_username(&username).await?;

        let Some((db_id, hash, created_at)) = found else {
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
            user: user_to_proto(id, &username, created_at).into(),
            ..Default::default()
        })
    }

    async fn get_me(
        &self,
        ctx: RequestContext,
        _req: ServiceRequest<'_, GetMeRequest>,
    ) -> ServiceResult<GetMeResponse> {
        let id = authenticate(&self.state, &ctx).await?;
        let (username, created_at) = self
            .state
            .session
            .execute_unpaged("SELECT username, created_at FROM users WHERE id = ?", (id_to_db(id),))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(String, DateTime<Utc>)>()
            .map_err(internal)?
            .ok_or_else(|| ConnectError::unauthenticated("user no longer exists"))?;
        Response::ok(GetMeResponse {
            user: user_to_proto(id, &username, created_at).into(),
            ..Default::default()
        })
    }
}

impl AuthSvc {
    /// Looks up `(user id, password hash, created_at)` by normalized username.
    async fn find_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<(i64, String, DateTime<Utc>)>, ConnectError> {
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
            .execute_unpaged("SELECT password_hash, created_at FROM users WHERE id = ?", (db_id,))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(String, DateTime<Utc>)>()
            .map_err(internal)?;
        Ok(row.map(|(hash, created_at)| (db_id, hash, created_at)))
    }
}
