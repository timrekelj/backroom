-- Channel look and layout. Channels created before this migration have all of
-- these columns null: they count as unsectioned (top level), position 0, no
-- icon/colour/topic, and are ordered by id (see layout::build_layout).
ALTER TABLE channels ADD section_id bigint;
ALTER TABLE channels ADD position int;
ALTER TABLE channels ADD icon text;
ALTER TABLE channels ADD color text;
ALTER TABLE channels ADD topic text;

CREATE TABLE sections (
    guild_id bigint,
    id bigint,
    name text,
    position int,
    PRIMARY KEY (guild_id, id)
);

CREATE TABLE roles (
    guild_id bigint,
    id bigint,
    name text,
    color text,
    position int,
    permissions bigint,
    created_at timestamp,
    PRIMARY KEY (guild_id, id)
);

-- One row per member that has roles; the set is bounded by the per-guild role
-- cap (roles::MAX_ROLES_PER_GUILD).
CREATE TABLE member_roles (
    guild_id bigint,
    user_id bigint,
    role_ids set<bigint>,
    PRIMARY KEY (guild_id, user_id)
);

-- Optimistic-concurrency counters ("layout", "roles"), only ever written with
-- lightweight transactions (never mixed with plain writes). A missing row is
-- version 0.
CREATE TABLE guild_versions (
    guild_id bigint,
    kind text,
    version bigint,
    PRIMARY KEY (guild_id, kind)
);
