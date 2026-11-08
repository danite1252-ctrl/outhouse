-- Removing old raid sign-up cards from chat once the raid is over.
--
-- card_cleanup: 'off' (the card stays), 'after' (3 hours after the raid
-- ends) or 'next' (repeating raids: when the next raid's card goes up, but
-- never before 3 hours after the end if that card went up before this raid
-- ended). Only the chat message goes: the raid, its sign-ups, attendance and
-- calendar entry stay.
ALTER TABLE event_series ADD COLUMN card_cleanup TEXT NOT NULL DEFAULT 'next';
ALTER TABLE events
    ADD COLUMN card_cleanup TEXT NOT NULL DEFAULT 'off',
    -- When the card went up (NULL: not yet).
    ADD COLUMN card_posted_at TIMESTAMPTZ,
    -- Set once the card has been removed (message_id is then NULL).
    ADD COLUMN card_removed_at TIMESTAMPTZ;

UPDATE events e SET card_posted_at = m.created_at FROM messages m WHERE m.id = e.message_id;

-- Existing repeating raids switch to the new default. Raids that ended more
-- than 8 days ago keep their cards, so history isn't swept away in one go.
UPDATE events SET card_cleanup = 'next'
WHERE series_id IS NOT NULL
  AND starts_at + make_interval(mins => duration_minutes) > now() - interval '8 days';

CREATE INDEX events_card_cleanup_idx ON events(starts_at)
    WHERE message_id IS NOT NULL AND card_cleanup <> 'off';

-- Outhouse settings → Raids: what "Plan a raid" picks to begin with.
ALTER TABLE raid_settings
    ADD COLUMN card_cleanup_series TEXT NOT NULL DEFAULT 'next',
    ADD COLUMN card_cleanup_single TEXT NOT NULL DEFAULT 'off';
