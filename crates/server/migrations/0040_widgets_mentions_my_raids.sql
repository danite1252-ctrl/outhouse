-- "My raids" across outhouses, right-hand widgets, realm time and the
-- Mentions inbox.

-- ---- My raids: raids in the channels you can see, by time --------------------
CREATE INDEX events_channel_starts_idx ON events(channel_id, starts_at);

-- ---- Sidebar widgets (each person's own, the same on every device) -----------
-- [{"id": "clock", "on": true, "collapsed": false}, {"id": "calendar", ...}]
-- in the order they stack above the member list. See api/widgets.rs.
CREATE TABLE user_widgets (
    user_id    BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    widgets    JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---- Realm time (Outhouse settings → Raids) -----------------------------------
-- realm_time_zone: an IANA zone ('' = work it out from the region).
-- realm_region: 'us', 'eu', 'kr' or 'tw' ('' = the server's Armory region,
-- when the Armory is switched on). The region also picks the weekly reset.
ALTER TABLE raid_settings
    ADD COLUMN realm_time_zone TEXT NOT NULL DEFAULT '',
    ADD COLUMN realm_region    TEXT NOT NULL DEFAULT '';

-- ---- Mentions inbox -------------------------------------------------------------
-- One row per person a message pings (directly, through one of their roles,
-- @everyone/@here, or a reply to them), written when the message is posted
-- (and kept in step when it's edited). Who can still see it is checked when
-- the inbox is read. Rows go with the message, and after 90 days.
CREATE TABLE mentions (
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    message_id BIGINT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    channel_id BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    -- 0: a direct message.
    guild_id   BIGINT NOT NULL,
    thread_id  BIGINT,
    author_id  BIGINT NOT NULL,
    -- 'user', 'role', 'everyone' or 'reply'.
    kind       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Marked read in the inbox (reading the channel past it counts too).
    read_at    TIMESTAMPTZ,
    PRIMARY KEY (user_id, message_id)
);
CREATE INDEX mentions_message_idx ON mentions(message_id);
CREATE INDEX mentions_unread_idx ON mentions(user_id, message_id) WHERE read_at IS NULL;
CREATE INDEX mentions_created_idx ON mentions(created_at);
