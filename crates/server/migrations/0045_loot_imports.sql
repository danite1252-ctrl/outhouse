-- Loot history from Gargul: raid leaders paste Gargul's "awarded loot"
-- export (CSV) and each award becomes a loot_awards row, matched to a raid
-- (by date and sign-ups) and to a member (by character name) where it can be.
-- Unmatched names and awards outside any raid are kept too.

-- Each paste is one import, which its importer can undo for 24 hours.
-- Soft reserves the import removed (the "remove it" rule) are kept here so
-- an undo can put them back.
CREATE TABLE loot_imports (
    id               BIGINT PRIMARY KEY,
    guild_id         BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    -- Imported from a raid's card (NULL: the outhouse's Loot page).
    event_id         BIGINT REFERENCES events(id) ON DELETE SET NULL,
    imported_by      BIGINT REFERENCES users(id) ON DELETE SET NULL,
    source           TEXT NOT NULL DEFAULT 'gargul',
    row_count        INT NOT NULL DEFAULT 0,
    removed_reserves JSONB NOT NULL DEFAULT '[]',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    undone_at        TIMESTAMPTZ
);
CREATE INDEX loot_imports_guild_idx ON loot_imports(guild_id, created_at DESC);

-- Awards no longer need a raid or a member: an unmatched character name, or
-- loot kept "without a raid", still counts.
ALTER TABLE loot_awards
    ALTER COLUMN user_id DROP NOT NULL,
    ALTER COLUMN channel_id DROP NOT NULL,
    ALTER COLUMN raid_starts_at DROP NOT NULL,
    ADD COLUMN gargul_id       TEXT,
    ADD COLUMN import_id       BIGINT REFERENCES loot_imports(id) ON DELETE CASCADE,
    -- The day it was handed out (in game); awarded_at is that day's best guess
    -- at a time when the export only had the date.
    ADD COLUMN awarded_on      DATE,
    -- Lower case, accents removed: what names are matched on.
    ADD COLUMN character_key   TEXT NOT NULL DEFAULT '',
    ADD COLUMN character_class TEXT NOT NULL DEFAULT '',
    ADD COLUMN offspec         BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN disenchanted    BOOLEAN NOT NULL DEFAULT false;

-- A member who leaves (or deletes their account) leaves their loot behind,
-- under the character's name.
ALTER TABLE loot_awards DROP CONSTRAINT IF EXISTS loot_awards_user_id_fkey;
ALTER TABLE loot_awards
    ADD CONSTRAINT loot_awards_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL;

UPDATE loot_awards SET awarded_on = (awarded_at AT TIME ZONE 'UTC')::date WHERE awarded_on IS NULL;
UPDATE loot_awards SET character_key = lower(character_name) WHERE character_key = '' AND character_name <> '';
ALTER TABLE loot_awards ALTER COLUMN awarded_on SET NOT NULL;
ALTER TABLE loot_awards ALTER COLUMN awarded_on SET DEFAULT CURRENT_DATE;

-- Gargul's award IDs: a re-import skips what's already in.
CREATE UNIQUE INDEX loot_awards_gargul_idx ON loot_awards(guild_id, gargul_id) WHERE gargul_id IS NOT NULL;
CREATE INDEX loot_awards_key_idx ON loot_awards(guild_id, character_key);
CREATE INDEX loot_awards_import_idx ON loot_awards(import_id) WHERE import_id IS NOT NULL;

-- Outhouse settings → Raids: what happens when someone wins the item they
-- soft-reserved. "keep" (nothing), "lock" (they can't reserve it again in
-- later raids of that instance unless a raid leader allows it) or "remove"
-- (the reserve is taken off the raid).
ALTER TABLE raid_settings ADD COLUMN sr_won TEXT NOT NULL DEFAULT 'lock';
