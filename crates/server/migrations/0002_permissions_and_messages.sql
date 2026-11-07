-- Roles and permissions, richer messages, uploads, link previews, unread state.

-- ---- Roles -------------------------------------------------------------
-- The @everyone role uses the server's own ID.
CREATE TABLE roles (
    id          BIGINT PRIMARY KEY,
    guild_id    BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    color       INT NOT NULL DEFAULT 0,
    permissions BIGINT NOT NULL DEFAULT 0,
    position    INT NOT NULL DEFAULT 0,
    hoist       BOOLEAN NOT NULL DEFAULT false,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX roles_guild_idx ON roles(guild_id);

-- Give every existing outhouse an @everyone role with the default permissions
-- (must match permissions::DEFAULT_EVERYONE = 459583).
INSERT INTO roles (id, guild_id, name, permissions, position)
SELECT id, id, '@everyone', 459583, 0 FROM guilds;

CREATE TABLE member_roles (
    guild_id BIGINT NOT NULL,
    user_id  BIGINT NOT NULL,
    role_id  BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    PRIMARY KEY (guild_id, user_id, role_id),
    FOREIGN KEY (guild_id, user_id) REFERENCES guild_members(guild_id, user_id) ON DELETE CASCADE
);

-- target_kind: 0 = role, 1 = member
CREATE TABLE channel_overwrites (
    channel_id  BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    target_id   BIGINT NOT NULL,
    target_kind SMALLINT NOT NULL,
    allow_bits  BIGINT NOT NULL DEFAULT 0,
    deny_bits   BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, target_id)
);

CREATE TABLE bans (
    guild_id   BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    reason     TEXT,
    banned_by  BIGINT REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id)
);

ALTER TABLE guild_members ADD COLUMN timeout_until TIMESTAMPTZ;

-- ---- Messages ----------------------------------------------------------
ALTER TABLE messages ADD COLUMN reply_to_id BIGINT;
ALTER TABLE channels ADD COLUMN last_message_id BIGINT;
UPDATE channels c SET last_message_id = (SELECT max(id) FROM messages m WHERE m.channel_id = c.id);

CREATE TABLE reactions (
    message_id BIGINT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    emoji      TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, user_id, emoji)
);

CREATE TABLE attachments (
    id           BIGINT PRIMARY KEY,
    message_id   BIGINT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    storage_key  TEXT NOT NULL UNIQUE,   -- random; also the file name on disk
    filename     TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size         BIGINT NOT NULL
);
CREATE INDEX attachments_message_idx ON attachments(message_id);

CREATE TABLE message_embeds (
    message_id   BIGINT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    position     INT NOT NULL,
    url          TEXT NOT NULL,
    kind         TEXT NOT NULL,          -- 'link' or 'image'
    title        TEXT,
    description  TEXT,
    site_name    TEXT,
    image_url    TEXT,
    PRIMARY KEY (message_id, position)
);

CREATE TABLE read_states (
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id   BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    last_read_id BIGINT NOT NULL,
    PRIMARY KEY (user_id, channel_id)
);
