-- DM channels are identified by the canonical participant pair (the two user
-- ids ordered ascending, joined with ':'). An LWT on the pair makes OpenDm
-- idempotent: concurrent opens by either user converge on one channel.
CREATE TABLE dm_channels_by_pair (
    pair text PRIMARY KEY,
    dm_id bigint
);

CREATE TABLE dm_channels (
    id bigint PRIMARY KEY,
    user_a bigint,
    user_b bigint,
    created_at timestamp
);

-- Per-user view of one's DM channels, newest first; also what the gateway
-- subscribes to when a stream opens.
CREATE TABLE dms_by_user (
    user_id bigint,
    dm_id bigint,
    created_at timestamp,
    PRIMARY KEY (user_id, dm_id)
) WITH CLUSTERING ORDER BY (dm_id DESC);

-- DM messages reuse the `messages` table, keyed by channel_id as always, with
-- guild_id = 0 marking them as DM messages.
