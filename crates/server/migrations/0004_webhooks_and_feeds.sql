-- Webhooks, RSS/Atom news feeds, bot authors and rich embeds.

-- Webhooks and feeds post as a "bot" user that can't sign in.
ALTER TABLE users ADD COLUMN is_bot BOOLEAN NOT NULL DEFAULT false;

-- A webhook can set a different name/avatar per message.
ALTER TABLE messages ADD COLUMN override_name TEXT;
ALTER TABLE messages ADD COLUMN override_avatar TEXT;

-- Rich (Discord-style) embeds.
ALTER TABLE message_embeds ADD COLUMN color INT;
ALTER TABLE message_embeds ADD COLUMN author_name TEXT;
ALTER TABLE message_embeds ADD COLUMN author_url TEXT;
ALTER TABLE message_embeds ADD COLUMN author_icon_url TEXT;
ALTER TABLE message_embeds ADD COLUMN footer TEXT;
ALTER TABLE message_embeds ADD COLUMN thumbnail_url TEXT;
ALTER TABLE message_embeds ADD COLUMN timestamp TEXT;
ALTER TABLE message_embeds ADD COLUMN fields JSONB;

CREATE TABLE webhooks (
    id          BIGINT PRIMARY KEY,
    guild_id    BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    channel_id  BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id     BIGINT NOT NULL REFERENCES users(id),
    name        TEXT NOT NULL,
    token       TEXT NOT NULL,
    creator_id  BIGINT REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX webhooks_guild_idx ON webhooks(guild_id);

CREATE TABLE feeds (
    id               BIGINT PRIMARY KEY,
    guild_id         BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    channel_id       BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id          BIGINT NOT NULL REFERENCES users(id),
    url              TEXT NOT NULL,
    title            TEXT NOT NULL,
    include_keywords TEXT NOT NULL DEFAULT '',
    exclude_keywords TEXT NOT NULL DEFAULT '',
    last_checked_at  TIMESTAMPTZ,
    last_error       TEXT,
    creator_id       BIGINT REFERENCES users(id),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX feeds_guild_idx ON feeds(guild_id);

-- Items already seen, so nothing gets posted twice.
CREATE TABLE feed_items (
    feed_id  BIGINT NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    guid     TEXT NOT NULL,
    seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (feed_id, guid)
);
