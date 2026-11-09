-- Raid loot lists read from Wowhead (zone, NPC and object pages), shared by
-- every outhouse on the server, plus item icons and richer tooltips.

-- One Wowhead page per branch ("classic", "tbc", "forever", …).
CREATE TABLE wowhead_lists (
    branch     TEXT NOT NULL,
    kind       TEXT NOT NULL,            -- zone | npc | object
    source_id  INT NOT NULL,
    name       TEXT NOT NULL DEFAULT '',
    -- ok, missing (Wowhead doesn't have it on this branch) or error (didn't answer)
    status     TEXT NOT NULL DEFAULT 'ok',
    error      TEXT NOT NULL DEFAULT '',
    -- Bosses in the order the page listed them.
    bosses     TEXT[] NOT NULL DEFAULT '{}',
    -- Last try, and the last time items were read (NULL = never).
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ok_at      TIMESTAMPTZ,
    PRIMARY KEY (branch, kind, source_id)
);

CREATE TABLE wowhead_drops (
    branch     TEXT NOT NULL,
    kind       TEXT NOT NULL,
    source_id  INT NOT NULL,
    item_id    INT NOT NULL,
    name       TEXT NOT NULL,
    quality    INT NOT NULL DEFAULT 1,
    icon       TEXT NOT NULL DEFAULT '',
    item_class INT NOT NULL DEFAULT -1,
    subclass   INT NOT NULL DEFAULT -1,
    slot       INT NOT NULL DEFAULT 0,
    req_class  INT NOT NULL DEFAULT 0,
    boss       TEXT NOT NULL DEFAULT '',
    boss_id    INT NOT NULL DEFAULT 0,
    chance     REAL,
    trash      BOOLEAN NOT NULL DEFAULT false,
    position   INT NOT NULL DEFAULT 0,
    PRIMARY KEY (branch, kind, source_id, item_id),
    FOREIGN KEY (branch, kind, source_id)
        REFERENCES wowhead_lists (branch, kind, source_id) ON DELETE CASCADE
);
CREATE INDEX wowhead_drops_item_idx ON wowhead_drops (item_id);

-- Wowhead pages a raid leader imported into one of the outhouse's loot
-- lists (by pasting a zone, NPC or object link).
CREATE TABLE loot_sources (
    guild_id   BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    instance   TEXT NOT NULL,
    branch     TEXT NOT NULL,
    kind       TEXT NOT NULL,
    source_id  INT NOT NULL,
    name       TEXT NOT NULL DEFAULT '',
    added_by   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, instance, branch, kind, source_id)
);

-- Item icons, kept so the lists look right when Wowhead is down. Empty
-- data = couldn't get it (tried again a day later).
CREATE TABLE wow_icons (
    name         TEXT PRIMARY KEY,
    content_type TEXT NOT NULL DEFAULT '',
    data         BYTEA NOT NULL DEFAULT '\x',
    fetched_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Tooltips per data set (Classic and TBC items differ) and as coloured
-- lines (JSON) for the app's own tooltips.
ALTER TABLE wow_items
    ADD COLUMN data_env TEXT NOT NULL DEFAULT '',
    ADD COLUMN lines TEXT NOT NULL DEFAULT '';
ALTER TABLE wow_items DROP CONSTRAINT wow_items_pkey;
ALTER TABLE wow_items ADD PRIMARY KEY (item_id, data_env);
