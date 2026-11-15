-- Join (and leave) messages: "{name} has joined the outhouse. Mind the gap."
--
-- A system message is a normal row in `messages` (its author is the person
-- who joined or left) with `system` set: which kind it is and which of the
-- templates was picked when it was posted, so it reads the same on reload.
--   {"kind": "member_join" | "member_leave" | "bot_join", "template": 3, "wave": true}
-- System messages can't be edited; only moderators can delete them.
ALTER TABLE messages ADD COLUMN system JSONB;

-- Outhouse settings → Overview → Welcome.
ALTER TABLE guilds
    -- Post a line when someone joins (default on).
    ADD COLUMN join_messages BOOLEAN NOT NULL DEFAULT TRUE,
    -- Where to post them. NULL: the first text channel @everyone can see.
    -- No foreign key: if the chosen channel is deleted, nothing is posted
    -- (rather than quietly moving to another channel).
    ADD COLUMN join_channel_id BIGINT,
    -- Post a line when someone leaves (default off).
    ADD COLUMN leave_messages BOOLEAN NOT NULL DEFAULT FALSE,
    -- Show a "Wave 👋" button under join messages.
    ADD COLUMN join_wave BOOLEAN NOT NULL DEFAULT TRUE;

-- When a join or leave line was last posted for someone, so leaving and
-- coming back a few times doesn't fill the channel (one per 5 minutes).
CREATE TABLE welcome_posts (
    guild_id   BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL CHECK (kind IN ('join', 'leave')),
    posted_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id, kind)
);
