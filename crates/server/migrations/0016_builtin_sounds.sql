-- The built-in "Outhouse classics" on every soundboard (files live in
-- web/sounds/builtin; ids look like builtin:flush). Favourites of them, the
-- ones an outhouse has hidden, and using one as your entrance sound.

CREATE TABLE builtin_sound_favourites (
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    sound_key  TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, sound_key)
);

CREATE TABLE hidden_builtin_sounds (
    guild_id   BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    sound_key  TEXT NOT NULL,
    hidden_by  BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, sound_key)
);

-- An entrance sound is an uploaded file or a built-in, never both.
ALTER TABLE entrance_sounds ALTER COLUMN storage_key DROP NOT NULL;
ALTER TABLE entrance_sounds ADD COLUMN builtin TEXT;
ALTER TABLE entrance_sounds ADD CONSTRAINT entrance_sounds_one_source
    CHECK ((storage_key IS NULL) <> (builtin IS NULL));
