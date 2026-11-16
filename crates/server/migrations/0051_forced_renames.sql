-- Usernames can't be changed by their owner any more. Staff (the new
-- `force_rename` permission) or the site owner can force a change when a
-- username is inappropriate: it's swapped for a neutral placeholder at once,
-- the old one is reserved for good, and the person picks a new one (once).

-- The person must choose a new username before they can use the app.
ALTER TABLE users ADD COLUMN must_choose_username BOOLEAN NOT NULL DEFAULT false;

-- Usernames nobody can take again (registration or the forced choice).
-- `name_key` is lower-case without accents (see api/names.rs).
CREATE TABLE reserved_usernames (
    name_key     TEXT PRIMARY KEY,
    username     TEXT NOT NULL,
    reason       TEXT NOT NULL,
    -- Whose username it was (kept so they can still sign in with it until
    -- they've picked a new one).
    user_id      BIGINT REFERENCES users(id) ON DELETE SET NULL,
    reserved_by  BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX reserved_usernames_user_idx ON reserved_usernames (user_id);

-- The site owner's blocked words for usernames (substrings, normalised:
-- lower-case, no accents, light leetspeak, no dots or underscores).
CREATE TABLE username_blocked_words (
    word        TEXT PRIMARY KEY,
    added_by    BIGINT REFERENCES users(id) ON DELETE SET NULL,
    added_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Name-history rows from a forced change (the bad name and the
-- placeholder) are for staff and the site owner, not on public profiles.
ALTER TABLE name_history ADD COLUMN staff_only BOOLEAN NOT NULL DEFAULT false;

-- Existing Moderators and Senior moderators get the new permission (their
-- tier's preset now includes it; Custom staff only if the owner ticks it).
UPDATE staff SET perms = array_append(perms, 'force_rename')
 WHERE tier IN ('moderator', 'senior') AND NOT ('force_rename' = ANY(perms));
