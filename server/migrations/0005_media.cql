-- Avatars and guild icons: opaque media keys ("" / null = none), served by
-- GET /media/{key}.
ALTER TABLE users ADD avatar text;
ALTER TABLE guilds ADD icon text;

-- One row per object in object storage, for the admin storage dashboard.
-- `guild_id` is set for objects that count against a guild (guild icons, later
-- emojis) and null for user-owned ones (avatars).
CREATE TABLE media_objects (
    key text PRIMARY KEY,
    owner_kind text,
    owner_id bigint,
    guild_id bigint,
    bytes bigint,
    created_at timestamp
);

-- Byte and object totals. Counters cannot share a table with regular columns.
-- `scope` is "global" or "guild:<id>".
CREATE TABLE media_totals (
    scope text PRIMARY KEY,
    bytes counter,
    objects counter
);
