-- Soundboard clips, one library per outhouse.
CREATE TABLE sounds (
    id           BIGINT PRIMARY KEY,
    guild_id     BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    emoji        TEXT NOT NULL DEFAULT '',
    storage_key  TEXT NOT NULL UNIQUE,
    content_type TEXT NOT NULL,
    size         BIGINT NOT NULL,
    volume       REAL NOT NULL DEFAULT 1.0,
    creator_id   BIGINT REFERENCES users(id),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX sounds_guild_idx ON sounds(guild_id);
