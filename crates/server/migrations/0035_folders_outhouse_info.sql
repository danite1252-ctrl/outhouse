-- Outhouse folders on the left rail, and an outhouse's "more info".

-- ---- Folders (each person's own, private) ------------------------------------
-- The rail's order: folders and outhouses outside folders share one list of
-- positions; outhouses in a folder are ordered within it.
CREATE TABLE guild_folders (
    id         BIGINT PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    -- One of the client's swatches (NULL: the default ember).
    color      TEXT,
    collapsed  BOOLEAN NOT NULL DEFAULT false,
    position   INT NOT NULL
);
CREATE INDEX guild_folders_user_idx ON guild_folders(user_id);

-- Where each of your outhouses sits. Leaving an outhouse (or being kicked,
-- banned, or it being deleted) removes its row with the membership.
CREATE TABLE guild_rail_positions (
    user_id    BIGINT NOT NULL,
    guild_id   BIGINT NOT NULL,
    folder_id  BIGINT REFERENCES guild_folders(id) ON DELETE SET NULL,
    position   INT NOT NULL,
    PRIMARY KEY (user_id, guild_id),
    FOREIGN KEY (guild_id, user_id) REFERENCES guild_members(guild_id, user_id) ON DELETE CASCADE
);
CREATE INDEX guild_rail_positions_folder_idx ON guild_rail_positions(folder_id);

-- ---- An outhouse's details (Outhouse settings → Overview) -----------------------
ALTER TABLE guilds
    ADD COLUMN icon_key    TEXT,
    ADD COLUMN banner_key  TEXT,
    ADD COLUMN tagline     TEXT NOT NULL DEFAULT '',
    ADD COLUMN description TEXT NOT NULL DEFAULT '',
    ADD COLUMN rules       TEXT NOT NULL DEFAULT '',
    -- [{"label": "...", "url": "https://..."}], at most 5.
    ADD COLUMN links       JSONB NOT NULL DEFAULT '[]';

-- Names an outhouse has had (members see the last few, for transparency).
CREATE TABLE guild_name_history (
    id          BIGINT PRIMARY KEY,
    guild_id    BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    old_name    TEXT NOT NULL,
    new_name    TEXT NOT NULL,
    changed_by  BIGINT REFERENCES users(id) ON DELETE SET NULL,
    changed_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX guild_name_history_guild_idx ON guild_name_history(guild_id, changed_at DESC);
