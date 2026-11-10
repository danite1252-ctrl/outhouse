-- Full profiles: a history of the names people have gone by, and DM
-- message requests (DMs from people who are neither your friend nor in an
-- outhouse with you are held until you accept them).

-- ---- Name history ------------------------------------------------------------------

-- Recorded by triggers, so every way a name can change is caught (the
-- account page, a moderator changing a nickname, the safety console, ...).
-- Nothing is ever deleted by the app; the profile page shows the newest 20.
CREATE TABLE name_history (
    id          BIGSERIAL PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- display_name, username or nickname
    kind        TEXT NOT NULL CHECK (kind IN ('display_name', 'username', 'nickname')),
    -- The outhouse a nickname was in (kept when the outhouse goes).
    guild_id    BIGINT,
    old_value   TEXT,
    new_value   TEXT,
    changed_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX name_history_user_idx ON name_history (user_id, changed_at DESC, id DESC);

CREATE FUNCTION record_user_name_change() RETURNS trigger AS $$
BEGIN
    IF NEW.display_name IS DISTINCT FROM OLD.display_name THEN
        INSERT INTO name_history (user_id, kind, old_value, new_value)
        VALUES (NEW.id, 'display_name', OLD.display_name, NEW.display_name);
    END IF;
    IF NEW.username IS DISTINCT FROM OLD.username THEN
        INSERT INTO name_history (user_id, kind, old_value, new_value)
        VALUES (NEW.id, 'username', OLD.username, NEW.username);
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

CREATE TRIGGER users_name_history
    AFTER UPDATE OF display_name, username ON users
    FOR EACH ROW EXECUTE FUNCTION record_user_name_change();

CREATE FUNCTION record_nickname_change() RETURNS trigger AS $$
BEGIN
    IF NEW.nickname IS DISTINCT FROM OLD.nickname THEN
        INSERT INTO name_history (user_id, kind, guild_id, old_value, new_value)
        VALUES (NEW.user_id, 'nickname', NEW.guild_id, OLD.nickname, NEW.nickname);
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

CREATE TRIGGER guild_members_name_history
    AFTER UPDATE OF nickname ON guild_members
    FOR EACH ROW EXECUTE FUNCTION record_nickname_change();

-- ---- Message requests -------------------------------------------------------------

-- Adults' choice for DMs from people they share no outhouse with (and
-- aren't friends with): held as requests, or refused outright.
ALTER TABLE users
    ADD COLUMN message_requests TEXT NOT NULL DEFAULT 'allow'
        CHECK (message_requests IN ('allow', 'block'));

-- One row per request. While a request is open (pending, or ignored, which
-- the sender isn't told about) the recipient isn't in the DM's
-- dm_participants, so nothing reaches them: no events, pushes, unread
-- counts or files. Accepting adds them.
CREATE TABLE dm_requests (
    id          BIGSERIAL PRIMARY KEY,
    channel_id  BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    from_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    to_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status      TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'accepted', 'ignored', 'blocked')),
    -- Messages sent while it was open (capped, see api::dms).
    messages    INT NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- When its first message went (the daily limit counts these).
    sent_at     TIMESTAMPTZ,
    decided_at  TIMESTAMPTZ,
    -- Accepted because they became friends or started sharing an outhouse.
    auto        BOOLEAN NOT NULL DEFAULT false
);
CREATE UNIQUE INDEX dm_requests_open_idx ON dm_requests (channel_id)
    WHERE status IN ('pending', 'ignored');
CREATE INDEX dm_requests_to_idx ON dm_requests (to_id) WHERE status = 'pending';
CREATE INDEX dm_requests_from_idx ON dm_requests (from_id, sent_at);
