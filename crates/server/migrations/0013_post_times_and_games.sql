-- Raid sign-up posting times, a game for raids and group posts, and game
-- types for WoW characters (WoW: Forever, TBC, Classic Era).

-- ---- When a raid's sign-up card goes up ------------------------------------
-- post_rule: {"mode": "now" | "days" | "weekday", "days", "weekday", "time",
-- "time_zone"}. NULL means straight away.
ALTER TABLE event_series ADD COLUMN post_rule JSONB;
ALTER TABLE events
    ADD COLUMN post_rule JSONB,
    -- When the card goes up, while it isn't up yet (NULL: straight away).
    ADD COLUMN post_at TIMESTAMPTZ;

-- Until now repeating raids got their card a week before: keep that for
-- existing series (it can be changed with "Edit this and future raids").
UPDATE event_series
SET post_rule = jsonb_build_object('mode', 'days', 'days', 7, 'time_zone', time_zone);
UPDATE events e
SET post_rule = jsonb_build_object('mode', 'days', 'days', 7,
                                   'time_zone', COALESCE(s.time_zone, 'UTC')),
    post_at = e.starts_at - interval '7 days'
FROM event_series s
WHERE e.message_id IS NULL AND s.id = e.series_id;
UPDATE events
SET post_rule = jsonb_build_object('mode', 'days', 'days', 7, 'time_zone', 'UTC'),
    post_at = starts_at - interval '7 days'
WHERE message_id IS NULL AND post_rule IS NULL;
CREATE INDEX events_post_due_idx ON events(post_at) WHERE message_id IS NULL;

-- ---- Which game a raid or group is for ("" = any) ---------------------------
ALTER TABLE event_series ADD COLUMN game TEXT NOT NULL DEFAULT '';
ALTER TABLE events ADD COLUMN game TEXT NOT NULL DEFAULT '';
ALTER TABLE lfg_groups ADD COLUMN game TEXT NOT NULL DEFAULT '';

-- ---- Character game types ----------------------------------------------------
ALTER TABLE wow_characters
    ADD COLUMN game TEXT NOT NULL DEFAULT 'forever',
    -- WoW: Forever characters have a first and a last name, and a ruleset
    -- instead of a realm.
    ADD COLUMN last_name TEXT NOT NULL DEFAULT '',
    ADD COLUMN ruleset TEXT NOT NULL DEFAULT '',
    ADD COLUMN race TEXT NOT NULL DEFAULT '';

-- Everyone so far played WoW: Forever. Where the "realm" looks like a ruleset,
-- use it as one.
UPDATE wow_characters SET ruleset = CASE lower(regexp_replace(realm, '[^A-Za-z]', '', 'g'))
        WHEN 'normal' THEN 'Normal' WHEN 'pve' THEN 'Normal'
        WHEN 'pvp' THEN 'PvP'
        WHEN 'rp' THEN 'Roleplay' WHEN 'roleplay' THEN 'Roleplay' WHEN 'roleplaying' THEN 'Roleplay'
        WHEN 'hc' THEN 'Hardcore' WHEN 'hardcore' THEN 'Hardcore'
        ELSE 'Normal' END;
-- Where the "realm" looks like a second name (one word of 2 to 12 letters
-- that isn't the game's or a ruleset's name), it becomes the last name.
-- Anything else stays out of sight in realm and the last name is left for
-- the player to fill in.
UPDATE wow_characters
SET last_name = upper(left(realm, 1)) || lower(substr(realm, 2)), realm = ''
WHERE realm ~ '^[[:alpha:]]{2,12}$'
  AND lower(realm) NOT IN ('forever', 'wow', 'wowforever', 'classic', 'era', 'normal', 'pve',
                           'pvp', 'rp', 'roleplay', 'roleplaying', 'hc', 'hardcore', 'realm',
                           'none', 'na', 'us', 'eu', 'oce', 'au');

DROP INDEX wow_characters_unique_idx;
CREATE UNIQUE INDEX wow_characters_unique_idx
    ON wow_characters(user_id, game, lower(name), lower(last_name), lower(realm));
CREATE INDEX wow_characters_last_name_idx ON wow_characters(lower(last_name)) WHERE last_name <> '';
