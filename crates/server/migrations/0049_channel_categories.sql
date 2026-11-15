-- Channel categories (as in Discord): a category is a channel of kind 4
-- with a name, a position and permission overwrites but no messages. Text
-- and voice channels point at one with parent_id (NULL: uncategorised).
-- Deleting a category leaves its channels uncategorised.
-- No outhouse gets categories made for it: without any, the channel list
-- still shows the old "TEXT CHANNELS" and "VOICE" headers.

ALTER TABLE channels
    ADD COLUMN parent_id BIGINT REFERENCES channels(id) ON DELETE SET NULL;
CREATE INDEX channels_parent_idx ON channels(parent_id) WHERE parent_id IS NOT NULL;

-- Categories each person has collapsed (on all their devices).
CREATE TABLE category_collapses (
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    category_id BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, category_id)
);
CREATE INDEX category_collapses_category_idx ON category_collapses(category_id);

-- Positions are renumbered in list order from now on (uncategorised text,
-- uncategorised voice, then each category with its text and voice
-- channels); start from today's order.
WITH ordered AS (
    SELECT id, row_number() OVER (PARTITION BY guild_id ORDER BY kind, position, id) - 1 AS pos
    FROM channels WHERE guild_id IS NOT NULL AND kind IN (0, 2)
)
UPDATE channels c SET position = o.pos FROM ordered o WHERE c.id = o.id;
