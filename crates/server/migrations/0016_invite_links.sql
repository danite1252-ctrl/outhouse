-- Invite links: list an outhouse's invites quickly (newest first).
CREATE INDEX IF NOT EXISTS invites_guild_idx ON invites (guild_id, created_at DESC);
