-- Friends, blocks and privacy settings; voice messages; stickers;
-- forwarded messages; scheduled messages.

-- ---- Friends and blocks ----------------------------------------------------

-- A friendship is stored once, smallest ID first.
CREATE TABLE friendships (
    user_a     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    user_b     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_a, user_b),
    CHECK (user_a < user_b)
);
CREATE INDEX friendships_b_idx ON friendships(user_b);

CREATE TABLE friend_requests (
    from_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    to_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (from_id, to_id),
    CHECK (from_id <> to_id)
);
CREATE INDEX friend_requests_to_idx ON friend_requests(to_id);

CREATE TABLE user_blocks (
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    blocked_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, blocked_id),
    CHECK (user_id <> blocked_id)
);
CREATE INDEX user_blocks_blocked_idx ON user_blocks(blocked_id);

-- Who may start a DM with you: 'everyone' (anyone in a shared outhouse) or
-- 'friends'. Who may send you a friend request: 'everyone', 'shared'
-- (people in your outhouses), 'friends_of_friends' or 'nobody'.
ALTER TABLE users
    ADD COLUMN dm_privacy TEXT NOT NULL DEFAULT 'everyone'
        CHECK (dm_privacy IN ('everyone', 'friends')),
    ADD COLUMN friend_request_privacy TEXT NOT NULL DEFAULT 'everyone'
        CHECK (friend_request_privacy IN ('everyone', 'shared', 'friends_of_friends', 'nobody'));

-- ---- Voice messages and forwarded attachments ------------------------------

-- A forwarded message points at the same stored file as the original, so
-- one file can now belong to several messages (it's only removed from disk
-- when the last one goes).
ALTER TABLE attachments DROP CONSTRAINT IF EXISTS attachments_storage_key_key;
CREATE INDEX attachments_storage_key_idx ON attachments(storage_key);
ALTER TABLE attachments
    ADD COLUMN voice       BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN duration_ms INT,
    ADD COLUMN waveform    TEXT;

-- ---- Stickers ----------------------------------------------------------------

CREATE TABLE stickers (
    id           BIGINT PRIMARY KEY,
    guild_id     BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    emoji        TEXT NOT NULL DEFAULT '',
    description  TEXT NOT NULL DEFAULT '',
    storage_key  TEXT NOT NULL UNIQUE,
    content_type TEXT NOT NULL,
    creator_id   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX stickers_guild_name_idx ON stickers(guild_id, lower(name));

-- A sticker message keeps a copy of which sticker it was ({id, name, url}),
-- and a forwarded message a snapshot of what was forwarded.
ALTER TABLE messages
    ADD COLUMN sticker JSONB,
    ADD COLUMN forward JSONB;

-- ---- Scheduled messages -----------------------------------------------------

-- No foreign key on the channel: if it's deleted the job tells the person
-- their message couldn't be sent instead of it silently vanishing.
CREATE TABLE scheduled_messages (
    id          BIGINT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id  BIGINT NOT NULL,
    guild_id    BIGINT NOT NULL DEFAULT 0,
    thread_id   BIGINT,
    content     TEXT NOT NULL,
    send_at     TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 'pending', 'sending' (claimed by the job) or 'failed'
    status      TEXT NOT NULL DEFAULT 'pending',
    error       TEXT
);
CREATE INDEX scheduled_messages_due_idx ON scheduled_messages(send_at) WHERE status = 'pending';
CREATE INDEX scheduled_messages_user_idx ON scheduled_messages(user_id, channel_id);
