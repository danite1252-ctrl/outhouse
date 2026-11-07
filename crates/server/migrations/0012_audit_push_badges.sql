-- Batch 6: audit log, push notifications, notification settings and badges.

-- ---- Audit log ------------------------------------------------------------
-- Moderation and admin actions in an outhouse. Old entries are pruned by the
-- server (AUDIT_RETENTION_DAYS, 90 by default).
CREATE TABLE audit_log (
    id          BIGINT PRIMARY KEY,
    guild_id    BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    -- Who did it (NULL if their account is gone).
    actor_id    BIGINT REFERENCES users(id) ON DELETE SET NULL,
    -- e.g. 'role_update', 'member_ban', 'message_delete'
    action      TEXT NOT NULL,
    -- 'channel', 'role', 'member', 'message', 'emoji', 'sound', 'bot',
    -- 'webhook', 'feed', 'raid', 'badge' or ''
    target_kind TEXT NOT NULL DEFAULT '',
    target_id   BIGINT,
    -- The target's name when it happened (it may be renamed or deleted later).
    target_name TEXT NOT NULL DEFAULT '',
    -- Human-readable before/after summary.
    details     TEXT NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_guild_idx ON audit_log(guild_id, id DESC);
CREATE INDEX audit_log_created_idx ON audit_log(created_at);

-- ---- Web Push -------------------------------------------------------------
-- The server's VAPID key pair (made on first start; PKCS#8 DER private key).
CREATE TABLE push_vapid (
    id            INT PRIMARY KEY CHECK (id = 1),
    private_pkcs8 BYTEA NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One row per browser/device that turned push on.
CREATE TABLE push_subscriptions (
    id          BIGINT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    endpoint    TEXT NOT NULL UNIQUE,
    p256dh      TEXT NOT NULL,
    auth        TEXT NOT NULL,
    user_agent  TEXT NOT NULL DEFAULT '',
    failures    INT NOT NULL DEFAULT 0,
    last_ok_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX push_subscriptions_user_idx ON push_subscriptions(user_id);

-- Per-outhouse notification level: 'all', 'mentions' (default) or 'nothing'.
-- guild_id 0 is direct messages ('all' by default).
CREATE TABLE notify_levels (
    user_id  BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    guild_id BIGINT NOT NULL,
    level    TEXT NOT NULL,
    PRIMARY KEY (user_id, guild_id)
);

-- Muted channels (no notifications at all from them).
CREATE TABLE channel_mutes (
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, channel_id)
);

-- ---- Badges ---------------------------------------------------------------
-- Custom badges an outhouse's admins hand out (automatic badges are worked
-- out on the fly).
CREATE TABLE badges (
    id          BIGINT PRIMARY KEY,
    guild_id    BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    emoji       TEXT NOT NULL,
    color       INT NOT NULL DEFAULT 0,
    description TEXT NOT NULL DEFAULT '',
    created_by  BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX badges_guild_idx ON badges(guild_id);

CREATE TABLE badge_awards (
    badge_id   BIGINT NOT NULL REFERENCES badges(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    awarded_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
    awarded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (badge_id, user_id)
);
CREATE INDEX badge_awards_user_idx ON badge_awards(user_id);
