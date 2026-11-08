-- Read state per thread (unread dots and mention counts on threads), and
-- mutes that wear off (channels and DMs: 15 minutes to 24 hours, or until
-- you turn it back on).

CREATE TABLE thread_reads (
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    thread_id    BIGINT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    last_read_id BIGINT NOT NULL,
    PRIMARY KEY (user_id, thread_id)
);
CREATE INDEX thread_reads_thread_idx ON thread_reads(thread_id);

-- Threads that already have replies start out read for everyone who could
-- see them, so the update doesn't light up every old thread at once.
INSERT INTO thread_reads (user_id, thread_id, last_read_id)
SELECT gm.user_id, t.id, t.last_message_id
FROM threads t
JOIN channels c ON c.id = t.channel_id
JOIN guild_members gm ON gm.guild_id = c.guild_id
WHERE t.last_message_id IS NOT NULL
ON CONFLICT DO NOTHING;

-- NULL: muted until turned back on.
ALTER TABLE channel_mutes ADD COLUMN muted_until TIMESTAMPTZ;

-- "Edit this and future raids" can change a repeating raid's days: the
-- series is split there, and the new part remembers the old one (so the old
-- part's last card still tidies itself away when the next raid's goes up).
ALTER TABLE event_series
    ADD COLUMN split_from BIGINT REFERENCES event_series(id) ON DELETE SET NULL;
