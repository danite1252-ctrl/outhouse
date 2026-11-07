-- Direct messages, threads, polls and custom emoji.

-- ---- Direct messages -----------------------------------------------------
-- A DM is a channel with no outhouse (kind 1 = one-to-one, 3 = group).
ALTER TABLE channels ALTER COLUMN guild_id DROP NOT NULL;
-- Who made a group DM.
ALTER TABLE channels ADD COLUMN owner_id BIGINT REFERENCES users(id) ON DELETE SET NULL;
-- "smallerId:largerId" for one-to-one DMs, so there's only ever one per pair.
ALTER TABLE channels ADD COLUMN dm_key TEXT UNIQUE;

CREATE TABLE dm_participants (
    channel_id BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    joined_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (channel_id, user_id)
);
CREATE INDEX dm_participants_user_idx ON dm_participants(user_id);

-- ---- Threads ---------------------------------------------------------------
-- Thread messages live in the parent channel's message table with thread_id
-- set, so permissions, reactions, edits and uploads all work the same way.
CREATE TABLE threads (
    id                 BIGINT PRIMARY KEY,
    channel_id         BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    -- The message it was started from (NULL: started from the channel, or that message was deleted).
    starter_message_id BIGINT UNIQUE REFERENCES messages(id) ON DELETE SET NULL,
    name               TEXT NOT NULL,
    creator_id         BIGINT REFERENCES users(id) ON DELETE SET NULL,
    message_count      INT NOT NULL DEFAULT 0,
    last_message_id    BIGINT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX threads_channel_idx ON threads(channel_id, id DESC);

ALTER TABLE messages ADD COLUMN thread_id BIGINT REFERENCES threads(id) ON DELETE CASCADE;
CREATE INDEX messages_thread_idx ON messages(thread_id, id DESC) WHERE thread_id IS NOT NULL;

-- ---- Polls -----------------------------------------------------------------
CREATE TABLE polls (
    message_id BIGINT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    question   TEXT NOT NULL,
    options    TEXT[] NOT NULL,
    multi      BOOLEAN NOT NULL DEFAULT false,
    ends_at    TIMESTAMPTZ,
    closed     BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX polls_open_idx ON polls(ends_at) WHERE NOT closed AND ends_at IS NOT NULL;

CREATE TABLE poll_votes (
    message_id BIGINT NOT NULL REFERENCES polls(message_id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    option_idx INT NOT NULL,
    voted_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, user_id, option_idx)
);

-- ---- Custom emoji ------------------------------------------------------------
CREATE TABLE emojis (
    id           BIGINT PRIMARY KEY,
    guild_id     BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    storage_key  TEXT NOT NULL UNIQUE,
    content_type TEXT NOT NULL,
    animated     BOOLEAN NOT NULL DEFAULT false,
    creator_id   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (guild_id, name)
);
