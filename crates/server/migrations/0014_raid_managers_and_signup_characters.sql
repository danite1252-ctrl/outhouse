-- Raid managers chosen by role, a default channel for raid sign-ups, and
-- characters on raid sign-ups.

-- ---- Raid settings (Outhouse settings → Raids) -------------------------------
-- Members with any of these roles run every raid in the outhouse (move people
-- between roster and bench, remove sign-ups, take attendance, edit and cancel
-- raids, export rosters) without needing Manage Events.
CREATE TABLE raid_manager_roles (
    guild_id BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    role_id  BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    PRIMARY KEY (guild_id, role_id)
);

CREATE TABLE raid_settings (
    guild_id           BIGINT PRIMARY KEY REFERENCES guilds(id) ON DELETE CASCADE,
    -- The channel "Plan a raid" picks to begin with (NULL: the one you're in).
    default_channel_id BIGINT REFERENCES channels(id) ON DELETE SET NULL
);

-- ---- Characters on sign-ups --------------------------------------------------
-- The character picked when signing up. The name and class are copied in, so
-- old rosters still read right after the character is renamed or removed
-- (character_id then goes NULL, the copy stays).
ALTER TABLE event_signups
    ADD COLUMN character_id BIGINT REFERENCES wow_characters(id) ON DELETE SET NULL,
    ADD COLUMN character_name TEXT NOT NULL DEFAULT '',
    ADD COLUMN character_last_name TEXT NOT NULL DEFAULT '',
    ADD COLUMN character_class TEXT NOT NULL DEFAULT '',
    ADD COLUMN character_game TEXT NOT NULL DEFAULT '';
CREATE INDEX event_signups_character_idx ON event_signups(character_id) WHERE character_id IS NOT NULL;
