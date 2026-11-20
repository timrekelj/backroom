-- Message attachments.
--
-- Lifecycle: POST /attachments streams the file to object storage and writes a
-- pending upload (valid for one hour, then Scylla's TTL drops the row).
-- SendMessage consumes the pending rows and writes `message_attachments`;
-- bytes are counted in media_objects / media_totals (kind 'attachment') at
-- that moment, never for pending uploads.

-- One row per unconsumed upload, owned by the uploader: `(uploader, id)` makes
-- "the caller's pending uploads" a single-partition read (outstanding count,
-- send-time lookup) and lets SendMessage consume up to ten of them in one
-- conditional batch. `id` is the future Attachment id. `key` / `thumbnail` are
-- media keys; `sweep_hour` locates the matching pending_sweep row.
CREATE TABLE pending_attachments (
    uploader bigint,
    id bigint,
    channel_id bigint,
    key text,
    filename text,
    content_type text,
    size bigint,
    width int,
    height int,
    thumbnail text,
    thumbnail_bytes bigint,
    sha256 text,
    expires_at timestamp,
    sweep_hour bigint,
    PRIMARY KEY ((uploader), id)
);

-- Orphan registry. TTL deletes pending rows silently, so the objects they
-- pointed at need a record that outlives them: one row per upload, written
-- BEFORE the object is created and removed only when the sweeper has deleted
-- the objects or SendMessage has consumed the upload. Partitioned by the hour
-- of expiry (unix hours) so the sweeper reads only buckets that can hold
-- expired rows. No TTL on purpose.
CREATE TABLE pending_sweep (
    hour bigint,
    id bigint,
    uploader bigint,
    key text,
    thumbnail text,
    expires_at timestamp,
    PRIMARY KEY ((hour), id)
);

-- Where the sweeper resumes after a restart: every hour bucket below it is empty.
CREATE TABLE sweep_cursor (
    name text PRIMARY KEY,
    hour bigint
);

-- Attachments of messages, partitioned like messages and reactions
-- ((channel_id, bucket), bucket = bucket_of(message_id)) so ListMessages reads
-- those of a page with one query per bucket. `position` keeps the order the
-- sender chose; `id` is the Attachment id. `file` / `thumbnail` are media
-- keys. `removed` is set (and the keys cleared) when an admin deletes the file.
CREATE TABLE message_attachments (
    channel_id bigint,
    bucket int,
    message_id bigint,
    position int,
    id bigint,
    filename text,
    content_type text,
    size bigint,
    file text,
    width int,
    height int,
    thumbnail text,
    sha256 text,
    removed boolean,
    PRIMARY KEY ((channel_id, bucket), message_id, position)
);
