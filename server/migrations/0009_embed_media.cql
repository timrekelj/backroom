-- Rich provider embeds (X, TikTok, Instagram, Reddit): author, kind, and the
-- images, videos and GIFs the server downloaded into S3.

-- What the embed shows: 'link' (default), 'image', 'video' or 'rich'.
ALTER TABLE message_embeds ADD kind text;
-- 'x', 'tiktok', 'instagram', 'reddit'; null for a generic link.
ALTER TABLE message_embeds ADD provider text;
ALTER TABLE message_embeds ADD author_name text;
ALTER TABLE message_embeds ADD author_url text;
-- Media key of a small WebP avatar (counted as kind 'embed').
ALTER TABLE message_embeds ADD author_icon text;
ALTER TABLE message_embeds ADD published_at timestamp;

-- Stored media of an embed, partitioned like `message_embeds` so ListMessages
-- reads those of a page with one query per bucket. `position` is the embed's
-- position in the message, `idx` the order of the media within the embed.
-- `file` and `poster` are media keys (`embeds/{channel_id}/{32 hex}.webp|mp4|webm`),
-- every one an object counted as kind 'embed'. `kind` is 'image', 'video' or 'gifv'.
CREATE TABLE message_embed_media (
    channel_id bigint,
    bucket int,
    message_id bigint,
    position int,
    idx int,
    kind text,
    file text,
    poster text,
    width int,
    height int,
    duration_ms int,
    size bigint,
    PRIMARY KEY ((channel_id, bucket), message_id, position, idx)
);

-- The per-URL cache also remembers what providers learn about the author.
ALTER TABLE embed_cache ADD author_url text;
ALTER TABLE embed_cache ADD author_icon text;
ALTER TABLE embed_cache ADD published_at bigint;
