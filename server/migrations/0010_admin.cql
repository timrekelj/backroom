-- Site administration: admin flag, analytics counters, file size index, audit log.

-- Site admins. `users.is_admin` is the flag every request checks (null =
-- false); `site_admins` lists the same users so "the last admin" can be found
-- without scanning `users`. Both are written together (see admin::set_admin).
ALTER TABLE users ADD is_admin boolean;

CREATE TABLE site_admins (
    kind text,
    user_id bigint,
    PRIMARY KEY (kind, user_id)
);

-- One-shot markers, e.g. "counters_backfilled" (see stats::backfill).
CREATE TABLE server_meta (
    name text PRIMARY KEY,
    value text
);

-- All analytics counters (see stats.rs); counters cannot share a table with
-- regular columns. `scope` is "site" (users, guilds, channels, messages),
-- "guild:<id>" (members, channels, messages), "user:<id>" (messages) or
-- "day:<YYYY-MM-DD>" (UTC; messages, signups, active_users).
CREATE TABLE counters (
    scope text,
    name text,
    value counter,
    PRIMARY KEY (scope, name)
);

-- "User was active on this UTC day" markers, written with
-- INSERT ... IF NOT EXISTS USING TTL (2 days) so each user is counted once per
-- day; the TTL drops them without sweeping.
CREATE TABLE daily_active (
    day text,
    user_id bigint,
    PRIMARY KEY (day, user_id)
);

-- Last time the user sent a message or opened a gateway stream.
CREATE TABLE user_activity (
    user_id bigint PRIMARY KEY,
    last_active_at timestamp
);

-- Where a stored object belongs, for the admin file list. Rows written before
-- this migration have these null (the object is still listed, without owner
-- details).
ALTER TABLE media_objects ADD filename text;
ALTER TABLE media_objects ADD content_type text;
ALTER TABLE media_objects ADD channel_id bigint;
ALTER TABLE media_objects ADD message_id bigint;
ALTER TABLE media_objects ADD user_id bigint;

-- Size-ordered index of media_objects: `scope` is "global", "global:<kind>",
-- "guild:<id>" or "guild:<id>:<kind>" (the media_totals scopes), `bucket` is
-- floor(log2(bytes)) capped at 40, which keeps partitions bounded; within a
-- partition the largest object comes first. A listing walks the buckets from
-- the top (see media::files).
CREATE TABLE files_by_size (
    scope text,
    bucket int,
    bytes bigint,
    key text,
    PRIMARY KEY ((scope, bucket), bytes, key)
) WITH CLUSTERING ORDER BY (bytes DESC, key ASC);

-- Admin audit log, partitioned by the 10-day bucket of the entry's Snowflake
-- id (like messages), newest first.
CREATE TABLE audit_log (
    bucket int,
    id bigint,
    actor_id bigint,
    action text,
    target text,
    reason text,
    PRIMARY KEY (bucket, id)
) WITH CLUSTERING ORDER BY (id DESC);
