-- Edit channel: topic, slowmode and content visibility (default, spoiler or
-- age-restricted), plus who has said "yes, I'm 18+" for each age-restricted
-- channel (kept on the server so it follows people across devices).

ALTER TABLE channels
    ADD COLUMN topic TEXT NOT NULL DEFAULT '',
    -- Seconds members wait between messages (0 = off). Manage Messages or
    -- Manage Channels skip it.
    ADD COLUMN slowmode_secs INT NOT NULL DEFAULT 0,
    -- 'default', 'spoiler' (messages blurred until clicked) or 'nsfw'
    -- (age-restricted: nothing is sent until the member accepts the gate).
    ADD COLUMN content_mode TEXT NOT NULL DEFAULT 'default';

ALTER TABLE channels
    ADD CONSTRAINT channels_topic_len CHECK (char_length(topic) <= 1024),
    ADD CONSTRAINT channels_slowmode_range CHECK (slowmode_secs BETWEEN 0 AND 21600),
    ADD CONSTRAINT channels_content_mode CHECK (content_mode IN ('default', 'spoiler', 'nsfw'));

-- The web client used to show this line on every text channel without it
-- being stored anywhere. It now belongs to the front porch only.
UPDATE channels SET topic = 'Leave it cleaner than you found it.'
WHERE kind = 0 AND name = 'front-porch' AND topic = '';

CREATE TABLE channel_age_accepts (
    channel_id  BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (channel_id, user_id)
);
CREATE INDEX channel_age_accepts_user_idx ON channel_age_accepts(user_id);

-- When each member last posted in a slowmode channel (deleting the message
-- doesn't reset the wait).
CREATE TABLE slowmode_posts (
    channel_id BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    posted_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (channel_id, user_id)
);
