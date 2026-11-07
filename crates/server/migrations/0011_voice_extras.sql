-- Voice extras: server mute/deafen that sticks, entrance sounds,
-- soundboard favourites, and the Share screen permission.

-- Server mute/deafen by a moderator, kept when people leave and rejoin.
CREATE TABLE voice_moderation (
    guild_id   BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    muted      BOOLEAN NOT NULL DEFAULT false,
    deafened   BOOLEAN NOT NULL DEFAULT false,
    updated_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id)
);

-- A short sound others hear when you join their voice channel. The file
-- itself is a profile file (served from /files like avatars).
CREATE TABLE entrance_sounds (
    user_id     BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    storage_key TEXT NOT NULL REFERENCES profile_files(storage_key) ON DELETE CASCADE,
    duration_ms INT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Everyone's favourite soundboard clips.
CREATE TABLE sound_favourites (
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    sound_id   BIGINT NOT NULL REFERENCES sounds(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, sound_id)
);

ALTER TABLE sounds ADD COLUMN duration_ms INT;

-- Share screen (1 << 22) for @everyone in existing outhouses, like new ones get.
UPDATE roles SET permissions = permissions | 4194304 WHERE id = guild_id;
