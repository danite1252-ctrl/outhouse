-- WoW tools: group finder (LFG) posts, WoW characters on profiles,
-- Battle.net linking, and a cache for Wowhead item lookups.

-- ---- Looking for group ---------------------------------------------------
CREATE TABLE lfg_groups (
    message_id    BIGINT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    leader_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    activity      TEXT NOT NULL,
    -- "dungeon", "raid" or "other".
    activity_kind TEXT NOT NULL DEFAULT 'other',
    min_level     INT,
    max_level     INT,
    max_tanks     INT NOT NULL,
    max_healers   INT NOT NULL,
    max_dps       INT NOT NULL,
    starts_at     TIMESTAMPTZ,
    note          TEXT NOT NULL DEFAULT '',
    closed        BOOLEAN NOT NULL DEFAULT false,
    expires_at    TIMESTAMPTZ NOT NULL,
    expired       BOOLEAN NOT NULL DEFAULT false,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX lfg_groups_expiry_idx ON lfg_groups(expires_at) WHERE NOT expired;

CREATE TABLE lfg_members (
    message_id BIGINT NOT NULL REFERENCES lfg_groups(message_id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role       TEXT NOT NULL,
    class      TEXT NOT NULL DEFAULT '',
    character  TEXT NOT NULL DEFAULT '',
    level      INT,
    joined_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, user_id)
);

-- ---- Characters ------------------------------------------------------------
CREATE TABLE wow_characters (
    id          BIGINT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    realm       TEXT NOT NULL DEFAULT '',
    class       TEXT NOT NULL DEFAULT '',
    level       INT NOT NULL DEFAULT 1,
    is_main     BOOLEAN NOT NULL DEFAULT false,
    professions TEXT[] NOT NULL DEFAULT '{}',
    armory_url  TEXT NOT NULL DEFAULT '',
    -- "manual" or "bnet" (imported from Battle.net).
    source      TEXT NOT NULL DEFAULT 'manual',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX wow_characters_user_idx ON wow_characters(user_id);
CREATE UNIQUE INDEX wow_characters_unique_idx ON wow_characters(user_id, lower(name), lower(realm));
CREATE INDEX wow_characters_name_idx ON wow_characters(lower(name));

-- ---- Battle.net ------------------------------------------------------------
CREATE TABLE bnet_links (
    user_id    BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    battletag  TEXT NOT NULL DEFAULT '',
    linked_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Sign-in attempts in progress (the "state" sent to Battle.net), kept ten minutes.
CREATE TABLE bnet_oauth_states (
    state      TEXT PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Must be sent again, unchanged, when swapping the code for a token.
    redirect_uri TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---- Wowhead item cache --------------------------------------------------
CREATE TABLE wow_items (
    item_id    INT PRIMARY KEY,
    name       TEXT NOT NULL,
    quality    INT NOT NULL DEFAULT 1,
    icon       TEXT NOT NULL DEFAULT '',
    tooltip    TEXT NOT NULL DEFAULT '',
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Name searches → item ID.
CREATE TABLE wow_item_searches (
    query      TEXT PRIMARY KEY,
    item_id    INT NOT NULL,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
