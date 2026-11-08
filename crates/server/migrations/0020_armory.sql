-- WoW: Forever characters on Blizzard's Armory: what it says about them
-- (read with the server's Battle.net keys), so cards can show the portrait,
-- guild and item level and keep the level fresh.
ALTER TABLE wow_characters
    -- "us", "eu", "kr" or "tw" ("" = not set; the server's region is used).
    ADD COLUMN region TEXT NOT NULL DEFAULT '',
    ADD COLUMN blizzard_id BIGINT,
    ADD COLUMN guild_name TEXT NOT NULL DEFAULT '',
    ADD COLUMN item_level INT,
    -- The portrait on Blizzard's render server, and our copy (wow_avatars).
    ADD COLUMN avatar_url TEXT NOT NULL DEFAULT '',
    ADD COLUMN avatar_key TEXT NOT NULL DEFAULT '',
    ADD COLUMN last_synced_at TIMESTAMPTZ,
    -- '' or 'not_found' (the Armory didn't know them last time).
    ADD COLUMN armory_status TEXT NOT NULL DEFAULT '';

CREATE INDEX wow_characters_blizzard_idx ON wow_characters(user_id, blizzard_id)
    WHERE blizzard_id IS NOT NULL;

-- Copies of Armory portraits, served from /api/wow/avatars/{key} so people's
-- browsers don't need to reach Blizzard (and cards keep working if it moves).
CREATE TABLE wow_avatars (
    key TEXT PRIMARY KEY,
    source_url TEXT NOT NULL,
    content_type TEXT NOT NULL,
    data BYTEA NOT NULL,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
