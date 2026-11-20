-- Invite-only accounts and authentication: emails, invites, one-time codes,
-- TOTP, recovery codes, passkeys and per-user session lists.

-- Account email (lowercased; null for legacy accounts), TOTP secret (base32;
-- null = off) and the last accepted TOTP time step (replay protection).
-- `password_hash` may now be null or "" for passwordless accounts.
ALTER TABLE users ADD email text;
ALTER TABLE users ADD totp_secret text;
ALTER TABLE users ADD totp_last_step bigint;

-- Unique claim of an email (INSERT ... IF NOT EXISTS), like users_by_username.
CREATE TABLE users_by_email (
    email text PRIMARY KEY,
    user_id bigint
);

-- Invites. Only hashes of the link token and the short code are stored.
-- `status` is pending | accepted | revoked (expired = pending past
-- expires_at). `guild_id` is the guild of a member's invite (null for site
-- admin invites); `guild_ids` are all guilds the new account joins.
CREATE TABLE invites (
    id bigint PRIMARY KEY,
    email text,
    token_hash text,
    code_hash text,
    inviter_id bigint,
    guild_id bigint,
    guild_ids list<bigint>,
    make_admin boolean,
    status text,
    expires_at timestamp,
    accepted_user_id bigint
);

-- Token hash or code hash -> invite.
CREATE TABLE invites_by_hash (
    hash text PRIMARY KEY,
    invite_id bigint
);

-- Listing index: `scope` is "all" or "guild:<id>", newest first.
CREATE TABLE invites_by_scope (
    scope text,
    id bigint,
    PRIMARY KEY (scope, id)
) WITH CLUSTERING ORDER BY (id DESC);

-- Emailed one-time codes, written with a 10-minute TTL. `purpose` is login,
-- reset or email_change; `subject` the email (login, reset) or the user id
-- (email_change, with the address in `new_email`).
CREATE TABLE email_codes (
    purpose text,
    subject text,
    code_hash text,
    attempts int,
    new_email text,
    PRIMARY KEY ((purpose, subject))
);

-- Hashed single-use recovery codes; deleted with DELETE ... IF EXISTS on use.
CREATE TABLE recovery_codes (
    user_id bigint,
    code_hash text,
    PRIMARY KEY (user_id, code_hash)
);

-- Passkeys: `credential` is the webauthn-rs Passkey as JSON.
CREATE TABLE passkeys (
    user_id bigint,
    id bigint,
    name text,
    credential text,
    created_at timestamp,
    last_used_at timestamp,
    PRIMARY KEY (user_id, id)
);

-- Sessions per user, written with the session's TTL. Sessions created before
-- this migration have no row here and are not listed (they still work).
ALTER TABLE sessions ADD id bigint;
ALTER TABLE sessions ADD device_name text;
CREATE TABLE sessions_by_user (
    user_id bigint,
    session_id bigint,
    token_hash text,
    device_name text,
    created_at timestamp,
    PRIMARY KEY (user_id, session_id)
);

-- Last use of a session (written at most every few minutes), with the
-- session's TTL.
CREATE TABLE session_seen (
    token_hash text PRIMARY KEY,
    last_seen_at timestamp
);
