-- Raid attendance from Method Raid Tools (MRT): raid leaders paste MRT's
-- attendance export (a grid of boss pulls × characters) and each column
-- becomes a pull on a raid, with who was there for it. Matched to members
-- by character name where it can be; unmatched names are kept as written.

-- Each paste is one import, which its importer can undo for 24 hours (and
-- raid managers can delete any time).
CREATE TABLE attendance_imports (
    id           BIGINT PRIMARY KEY,
    guild_id     BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    -- Imported from a raid's card (NULL: the outhouse's Loot & attendance window).
    event_id     BIGINT REFERENCES events(id) ON DELETE SET NULL,
    imported_by  BIGINT REFERENCES users(id) ON DELETE SET NULL,
    source       TEXT NOT NULL DEFAULT 'mrt',
    boss_count   INT NOT NULL DEFAULT 0,
    player_count INT NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    undone_at    TIMESTAMPTZ
);
CREATE INDEX attendance_imports_guild_idx ON attendance_imports(guild_id, created_at DESC);

-- One boss pull (a column of the export) on a raid.
CREATE TABLE attendance_pulls (
    id            BIGINT PRIMARY KEY,
    guild_id      BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    event_id      BIGINT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    import_id     BIGINT NOT NULL REFERENCES attendance_imports(id) ON DELETE CASCADE,
    -- As MRT wrote it, without the difficulty number.
    boss_name     TEXT NOT NULL,
    -- What re-imports are matched on: the encounter key, or "name:<boss>".
    boss_key      TEXT NOT NULL,
    -- "bt:highwarlordnajentus" when the boss is on the encounter lists.
    encounter_key TEXT,
    -- MRT's difficulty ID (3 = 10-player, 4 = 25-player, 9 = 40-player).
    difficulty    INT,
    -- The exporter's local time, and that as an instant in the raid's zone.
    pulled_local  TIMESTAMP NOT NULL,
    pulled_at     TIMESTAMPTZ NOT NULL
);
CREATE UNIQUE INDEX attendance_pulls_dedupe_idx ON attendance_pulls(event_id, boss_key, pulled_local);
CREATE INDEX attendance_pulls_import_idx ON attendance_pulls(import_id);
CREATE INDEX attendance_pulls_guild_idx ON attendance_pulls(guild_id, pulled_at);

-- Who was listed under a pull, and whether they were there for it.
CREATE TABLE attendance_marks (
    pull_id         BIGINT NOT NULL REFERENCES attendance_pulls(id) ON DELETE CASCADE,
    event_id        BIGINT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    character_name  TEXT NOT NULL,
    -- Lower case, accents and realm removed: what names are matched on.
    character_key   TEXT NOT NULL,
    character_class TEXT NOT NULL DEFAULT '',
    -- The member it matched (NULL: nobody here has a character by that name).
    user_id         BIGINT REFERENCES users(id) ON DELETE SET NULL,
    present         BOOLEAN NOT NULL,
    PRIMARY KEY (pull_id, character_key)
);
CREATE INDEX attendance_marks_event_idx ON attendance_marks(event_id, user_id);
CREATE INDEX attendance_marks_user_idx ON attendance_marks(user_id) WHERE user_id IS NOT NULL;
