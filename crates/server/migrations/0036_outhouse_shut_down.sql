-- The site owner's Outhouses directory: shutting an outhouse down (frozen,
-- closed on members' rails for a grace period, then members removed; data
-- kept as evidence until it's deleted for good), and safety log entries
-- about an outhouse.

ALTER TABLE guilds
    ADD COLUMN shut_down_at     TIMESTAMPTZ,
    ADD COLUMN shut_down_by     BIGINT,
    ADD COLUMN shut_down_reason TEXT NOT NULL DEFAULT '',
    -- Members keep it on their rail (closed) until then, so they can leave
    -- on their own; then they're removed (SHUTDOWN_GRACE_DAYS, default 7).
    ADD COLUMN members_remove_at  TIMESTAMPTZ,
    ADD COLUMN members_removed_at TIMESTAMPTZ,
    -- Who was in it when members were removed (kept as evidence):
    -- [{"user_id", "username", "display_name", "age_bracket", "joined_at", "role_ids", "roles"}]
    ADD COLUMN member_snapshot    JSONB;

-- Which outhouse a safety log entry is about (NULL: none in particular).
ALTER TABLE safety_log ADD COLUMN guild_id BIGINT;
CREATE INDEX safety_log_guild_idx ON safety_log(guild_id) WHERE guild_id IS NOT NULL;
CREATE INDEX reports_guild_idx ON reports(guild_id) WHERE guild_id IS NOT NULL;
