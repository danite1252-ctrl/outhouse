-- Profiles, raid sign-ups, bots with slash commands.

-- ---- Profiles ----------------------------------------------------------
ALTER TABLE users ADD COLUMN avatar_key    TEXT;
ALTER TABLE users ADD COLUMN banner_key    TEXT;
ALTER TABLE users ADD COLUMN about_me      TEXT NOT NULL DEFAULT '';
-- One of the built-in profile backgrounds ('' = plain).
ALTER TABLE users ADD COLUMN profile_theme TEXT NOT NULL DEFAULT '';
-- 0xRRGGBB, 0 = default.
ALTER TABLE users ADD COLUMN accent_color  INT NOT NULL DEFAULT 0;

-- Uploaded avatar and banner images (served by /files like attachments).
CREATE TABLE profile_files (
    storage_key  TEXT PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename     TEXT NOT NULL,
    content_type TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---- Raid planner -------------------------------------------------------
CREATE TABLE events (
    id               BIGINT PRIMARY KEY,
    guild_id         BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    channel_id       BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    -- The chat message that shows the sign-up card. Deleting it deletes the event.
    message_id       BIGINT NOT NULL UNIQUE REFERENCES messages(id) ON DELETE CASCADE,
    creator_id       BIGINT NOT NULL REFERENCES users(id),
    title            TEXT NOT NULL,
    description      TEXT NOT NULL DEFAULT '',
    starts_at        TIMESTAMPTZ NOT NULL,
    duration_minutes INT NOT NULL DEFAULT 180,
    max_tanks        INT NOT NULL DEFAULT 2,
    max_healers      INT NOT NULL DEFAULT 4,
    max_dps          INT NOT NULL DEFAULT 14,
    reminded         BOOLEAN NOT NULL DEFAULT false,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX events_guild_idx ON events(guild_id, starts_at);
CREATE INDEX events_reminder_idx ON events(starts_at) WHERE NOT reminded;

CREATE TABLE event_signups (
    event_id  BIGINT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    user_id   BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 'yes', 'late', 'tentative', 'bench' or 'no'
    status    TEXT NOT NULL,
    -- 'tank', 'healer', 'dps' or ''
    role      TEXT NOT NULL DEFAULT '',
    class     TEXT NOT NULL DEFAULT '',
    spec      TEXT NOT NULL DEFAULT '',
    note      TEXT NOT NULL DEFAULT '',
    signed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (event_id, user_id)
);

-- Each outhouse gets a helper bot for reminders, made the first time it's needed.
ALTER TABLE guilds ADD COLUMN helper_bot_id BIGINT REFERENCES users(id);

-- ---- Bots ---------------------------------------------------------------
-- A bot is a user (is_bot) with a token instead of a password.
CREATE TABLE bots (
    user_id    BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    owner_id   BIGINT NOT NULL REFERENCES users(id),
    -- The outhouse it was made in (it can be invited to others).
    guild_id   BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Slash commands bots register per outhouse.
CREATE TABLE commands (
    id          BIGINT PRIMARY KEY,
    guild_id    BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    bot_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    options     JSONB NOT NULL DEFAULT '[]',
    UNIQUE (guild_id, name)
);

-- "Alice used /roll" above a bot's reply.
ALTER TABLE messages ADD COLUMN interaction JSONB;
