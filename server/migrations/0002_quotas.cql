-- Atomic creation quotas. Counters are incremented first and compensated on
-- rejection (see GuildSvc::reserve_guild_slot / reserve_channel_slot), so
-- concurrent requests cannot collectively bypass the caps.
CREATE TABLE user_guild_quota (
    user_id bigint PRIMARY KEY,
    created counter
);

CREATE TABLE guild_channel_quota (
    guild_id bigint PRIMARY KEY,
    created counter
);
