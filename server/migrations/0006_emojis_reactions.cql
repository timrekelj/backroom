-- Custom emojis, three-way channel icons and message reactions.

-- Emojis of a guild (at most 200, enforced in the service). `image` is the
-- media key (emojis/{guild}/{hash}.webp|gif); `animated` = stored as GIF.
CREATE TABLE emojis (
    guild_id bigint,
    id bigint,
    name text,
    animated boolean,
    image text,
    creator_id bigint,
    PRIMARY KEY (guild_id, id)
);

-- Name claims: one row per (guild, name), written with an LWT
-- (INSERT ... IF NOT EXISTS) so concurrent creates/renames cannot end up with
-- two emojis of the same name.
CREATE TABLE emoji_names (
    guild_id bigint,
    name text,
    emoji_id bigint,
    PRIMARY KEY (guild_id, name)
);

-- Per-kind byte/object totals live in media_totals under the scopes
-- "global:<kind>" and "guild:<id>:<kind>" (kind = avatar, guild_icon, emoji,
-- later attachment, embed); the existing "global" and "guild:<id>" scopes stay
-- the sum over all kinds. `kind` on the object row feeds admin file listings;
-- rows written before this migration have it null (derive it from the key).
ALTER TABLE media_objects ADD kind text;

-- Authoritative channel icon: icon_kind is 'phosphor', 'unicode' or 'custom'
-- (icon_value is the name, the emoji, or the decimal emoji id), or 'none' for
-- an explicit default. Null = never set with the new API: fall back to the
-- legacy `icon` column (phosphor name, "" = default). `icon` is always kept as
-- the phosphor mirror.
ALTER TABLE channels ADD icon_kind text;
ALTER TABLE channels ADD icon_value text;

-- Reactions, one row per (message, emoji, user). Partitioned like messages
-- ((channel_id, bucket) with bucket = bucket_of(message_id)) so ListMessages
-- reads the reactions of a page with one query per bucket it touched.
-- emoji_key is "u:<unicode>" or "c:<custom emoji id>"; `at` is a snowflake
-- stamped on every row of an emoji with the time the emoji was first used on the
-- message (copied from an existing row), giving a stable order.
CREATE TABLE reactions (
    channel_id bigint,
    bucket int,
    message_id bigint,
    emoji_key text,
    user_id bigint,
    at bigint,
    PRIMARY KEY ((channel_id, bucket), message_id, emoji_key, user_id)
);
