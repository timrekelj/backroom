-- Link embeds (server-side unfurling of URLs in messages).
--
-- After SendMessage commits, a background job fetches up to three URLs of the
-- message and writes `message_embeds`, then publishes `message_updated`. If
-- nothing useful was found nothing is written.

-- Embeds of messages, partitioned like messages, reactions and attachments
-- ((channel_id, bucket), bucket = bucket_of(message_id)) so ListMessages reads
-- those of a page with one query per bucket. `position` is the order of the
-- URLs in the message. `thumbnail` is a media key (`embeds/{channel_id}/{id}.webp`,
-- an object counted as kind 'embed'); it is null when the page had no usable
-- image or an admin deleted the file.
CREATE TABLE message_embeds (
    channel_id bigint,
    bucket int,
    message_id bigint,
    position int,
    url text,
    title text,
    description text,
    site_name text,
    thumbnail text,
    thumbnail_width int,
    thumbnail_height int,
    PRIMARY KEY ((channel_id, bucket), message_id, position)
);

-- Per-URL cache of what unfurling a URL found, so popular links are not
-- fetched again. Rows carry a TTL (the cache lifetime), so there is no
-- sweeping. Only metadata and the *source URLs* of the media are cached
-- (`media`: JSON array of {kind, url, width, height}), never stored objects:
-- every embed owns its own stored files (own keys, own accounting rows,
-- deleted with their owner), which keeps storage accounting and admin
-- deletion free of sharing rules. `found = false` is a negative entry
-- (nothing useful on the page).
CREATE TABLE embed_cache (
    url text PRIMARY KEY,
    found boolean,
    title text,
    description text,
    site_name text,
    author text,
    media text
);
