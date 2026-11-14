-- Site staff (platform-level helpers for the site owner) and outhouse
-- ownership transfers. See api/staff.rs and api/transfers.rs.

-- A session that passed two-factor (the code step, or a trusted device).
-- Staff powers only work in such a session.
ALTER TABLE sessions ADD COLUMN mfa BOOLEAN NOT NULL DEFAULT false;

-- When someone last used Outhouse (noted at most once an hour), for
-- "the owner hasn't been active for 90 days" ownership transfers.
ALTER TABLE users ADD COLUMN last_active_at TIMESTAMPTZ;
UPDATE users u SET last_active_at = GREATEST(
    u.created_at,
    (SELECT max(last_seen) FROM user_ips i WHERE i.user_id = u.id),
    (SELECT max(m.created_at) FROM messages m WHERE m.author_id = u.id));
-- Suspension notices say "by Outhouse staff" (never the person) when staff did it.
ALTER TABLE users ADD COLUMN suspended_by_staff BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE staff (
    user_id        BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    -- support | moderator | senior | custom
    tier           TEXT NOT NULL,
    perms          TEXT[] NOT NULL DEFAULT '{}',
    added_by       BIGINT REFERENCES users(id) ON DELETE SET NULL,
    added_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_active_at TIMESTAMPTZ,
    -- Set while their powers are suspended (two-factor off or email not
    -- confirmed); the owner has been told.
    lapsed_at      TIMESTAMPTZ,
    lapsed_reason  TEXT
);

-- Hourly action caps: one row per counted action.
CREATE TABLE staff_actions (
    id      BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL,
    heavy   BOOLEAN NOT NULL,
    at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX staff_actions_user_idx ON staff_actions(user_id, at);

-- Two-person rule: a senior moderator's big action waits for a second person.
CREATE TABLE staff_requests (
    id             BIGINT PRIMARY KEY,
    kind           TEXT NOT NULL CHECK (kind IN ('suspend_permanent_ip', 'shut_down')),
    requested_by   BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_user_id BIGINT,
    guild_id       BIGINT,
    report_id      BIGINT,
    payload        JSONB NOT NULL DEFAULT '{}',
    summary        TEXT NOT NULL DEFAULT '',
    status         TEXT NOT NULL DEFAULT 'pending'
                   CHECK (status IN ('pending', 'approved', 'declined', 'expired', 'cancelled', 'failed')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at     TIMESTAMPTZ NOT NULL,
    decided_by     BIGINT REFERENCES users(id) ON DELETE SET NULL,
    decided_at     TIMESTAMPTZ,
    decision_note  TEXT NOT NULL DEFAULT '',
    result         TEXT NOT NULL DEFAULT ''
);
CREATE INDEX staff_requests_status_idx ON staff_requests(status, created_at);

ALTER TABLE site_settings
    ADD COLUMN two_person_permanent_ip BOOLEAN NOT NULL DEFAULT true,
    ADD COLUMN two_person_shut_down BOOLEAN NOT NULL DEFAULT true,
    ADD COLUMN two_person_members INT NOT NULL DEFAULT 25,
    ADD COLUMN staff_badge BOOLEAN NOT NULL DEFAULT false;

-- Who did a staff action, as which tier, using which permission.
ALTER TABLE safety_log
    ADD COLUMN staff_tier TEXT,
    ADD COLUMN staff_perm TEXT;
CREATE INDEX safety_log_staff_idx ON safety_log(actor_id, created_at) WHERE staff_tier IS NOT NULL;

-- Staff entries in the safety log can't be changed or deleted.
CREATE FUNCTION safety_log_staff_locked() RETURNS trigger AS $$
BEGIN
    IF OLD.staff_tier IS NOT NULL THEN
        RAISE EXCEPTION 'staff entries in the safety log cannot be changed';
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER safety_log_staff_locked BEFORE UPDATE OR DELETE ON safety_log
    FOR EACH ROW EXECUTE FUNCTION safety_log_staff_locked();

-- Handing an outhouse to someone else.
CREATE TABLE ownership_transfers (
    id                  BIGINT PRIMARY KEY,
    guild_id            BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    from_user           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    to_user             BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- owner: the owner hands it over; inactive: staff, the owner has been
    -- away 90+ days; verified: staff, the owner confirmed by email.
    route               TEXT NOT NULL CHECK (route IN ('owner', 'inactive', 'verified')),
    -- awaiting_owner (verified: the email link), objection (inactive: the
    -- owner's window to say "I'm still here"), awaiting_recipient, then done.
    status              TEXT NOT NULL CHECK (status IN ('awaiting_owner', 'objection', 'awaiting_recipient',
                                                        'completed', 'declined', 'cancelled', 'expired', 'objected')),
    keep_admin          BOOLEAN NOT NULL DEFAULT false,
    allow_teen          BOOLEAN NOT NULL DEFAULT false,
    staff_id            BIGINT REFERENCES users(id) ON DELETE SET NULL,
    reason              TEXT NOT NULL DEFAULT '',
    recipient_accepted_at TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- When the step it's waiting on runs out.
    deadline            TIMESTAMPTZ NOT NULL,
    objection_until     TIMESTAMPTZ,
    owner_token_hash    TEXT,
    owner_confirmed_at  TIMESTAMPTZ,
    finished_at         TIMESTAMPTZ,
    finished_by         BIGINT REFERENCES users(id) ON DELETE SET NULL,
    outcome             TEXT NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX ownership_transfers_one_pending ON ownership_transfers(guild_id)
    WHERE status IN ('awaiting_owner', 'objection', 'awaiting_recipient');
CREATE INDEX ownership_transfers_to_idx ON ownership_transfers(to_user, status);
CREATE INDEX ownership_transfers_from_idx ON ownership_transfers(from_user, status);
