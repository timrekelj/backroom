CREATE TABLE users (
    id bigint PRIMARY KEY,
    username text,
    password_hash text,
    created_at timestamp
);

CREATE TABLE users_by_username (
    username text PRIMARY KEY,
    user_id bigint
);

CREATE TABLE sessions (
    token_hash text PRIMARY KEY,
    user_id bigint,
    created_at timestamp
);

CREATE TABLE guilds (
    id bigint PRIMARY KEY,
    name text,
    owner_id bigint,
    invite_code text,
    created_at timestamp
);

CREATE TABLE guilds_by_invite (
    invite_code text PRIMARY KEY,
    guild_id bigint
);

CREATE TABLE guild_members (
    guild_id bigint,
    user_id bigint,
    joined_at timestamp,
    PRIMARY KEY (guild_id, user_id)
);

CREATE TABLE guilds_by_user (
    user_id bigint,
    guild_id bigint,
    joined_at timestamp,
    PRIMARY KEY (user_id, guild_id)
);

CREATE TABLE channels (
    guild_id bigint,
    id bigint,
    name text,
    created_at timestamp,
    PRIMARY KEY (guild_id, id)
);

CREATE TABLE channels_by_id (
    id bigint PRIMARY KEY,
    guild_id bigint
);

-- `bucket` is a 10-day window derived from the message's Snowflake timestamp
-- (see snowflake::bucket_of). Partitioning by (channel_id, bucket) keeps each
-- partition bounded no matter how busy a channel gets.
CREATE TABLE messages (
    channel_id bigint,
    bucket int,
    id bigint,
    guild_id bigint,
    author_id bigint,
    content text,
    nonce text,
    edited_at timestamp,
    PRIMARY KEY ((channel_id, bucket), id)
) WITH CLUSTERING ORDER BY (id DESC);
