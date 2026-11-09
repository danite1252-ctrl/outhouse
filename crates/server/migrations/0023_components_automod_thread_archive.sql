-- Bot message components (buttons and select menus, Discord's JSON shape),
-- per-channel automod, and threads that hide themselves after a while
-- without replies (auto-archive).

-- Action rows of buttons / select menus on a bot's message (NULL: none).
ALTER TABLE messages ADD COLUMN components JSONB;
-- Automod let it through but hides it behind a "Filtered" reveal for others.
ALTER TABLE messages ADD COLUMN filtered BOOLEAN NOT NULL DEFAULT FALSE;

-- Automod settings for one channel (see automod.rs for the fields).
CREATE TABLE automod_channels (
    channel_id BIGINT PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
    settings   JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The outhouse-wide blocked words channels can use, and the "Automod" bot
-- account that posts alerts to a mod-log channel.
CREATE TABLE automod_guilds (
    guild_id    BIGINT PRIMARY KEY REFERENCES guilds(id) ON DELETE CASCADE,
    settings    JSONB NOT NULL,
    bot_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Threads: hide after this many minutes without activity (1 h, 24 h,
-- 3 days or 1 week), when they last had activity, archived and locked.
ALTER TABLE threads
    ADD COLUMN auto_archive_mins INT NOT NULL DEFAULT 4320,
    ADD COLUMN active_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ADD COLUMN archived_at TIMESTAMPTZ,
    ADD COLUMN locked BOOLEAN NOT NULL DEFAULT FALSE;
-- Existing threads count from their last reply (or when they were started).
UPDATE threads t SET active_at = COALESCE(
    (SELECT max(m.created_at) FROM messages m WHERE m.thread_id = t.id), t.created_at);
CREATE INDEX threads_active_idx ON threads(active_at) WHERE archived_at IS NULL;

-- A channel's default for new threads ("Hide after inactivity").
ALTER TABLE channels ADD COLUMN thread_archive_mins INT NOT NULL DEFAULT 4320;
