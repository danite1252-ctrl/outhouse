-- Safety: reports to the site owner, evidence snapshots, quarantined files,
-- account suspensions, age groups and teen protections.

-- ---- Accounts --------------------------------------------------------------

-- Only an age group is kept, never the date of birth. Accounts made before
-- this count as adults; the site owner can change it.
ALTER TABLE users
    ADD COLUMN age_bracket TEXT NOT NULL DEFAULT 'adult'
        CHECK (age_bracket IN ('teen', 'adult')),
    -- Suspended site-wide until then (9999-12-31 = permanently).
    ADD COLUMN suspended_until TIMESTAMPTZ,
    ADD COLUMN suspended_reason TEXT;

-- Outhouses the site owner marks as family: teens can get friend requests
-- from people they share one with.
ALTER TABLE guilds ADD COLUMN family BOOLEAN NOT NULL DEFAULT false;

-- Hidden from everyone pending review (child-safety reports, or all of a
-- suspended person's messages). Kept for evidence.
ALTER TABLE messages ADD COLUMN safety_hidden BOOLEAN NOT NULL DEFAULT false;
CREATE INDEX messages_safety_hidden_idx ON messages(author_id) WHERE safety_hidden;

-- ---- Site settings (one row) -------------------------------------------------

CREATE TABLE site_settings (
    id                         SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    -- Teens: DMs and friend requests only from people they share an outhouse
    -- with who are friends of friends or in a family outhouse; adults can't
    -- start DMs with teens they aren't friends with.
    teen_contact_limits        BOOLEAN NOT NULL DEFAULT true,
    -- Teens never see age-restricted (18+) channels.
    teen_block_adult_channels  BOOLEAN NOT NULL DEFAULT true,
    -- Sign-up asks for a date of birth (under 13s can't make an account).
    require_birth_date         BOOLEAN NOT NULL DEFAULT true,
    -- The "Outhouse Safety" account that warnings and digests come from.
    safety_bot_id              BIGINT REFERENCES users(id) ON DELETE SET NULL,
    last_digest_at             TIMESTAMPTZ
);
INSERT INTO site_settings (id) VALUES (1);

-- ---- Reports -------------------------------------------------------------------

CREATE TABLE reports (
    id               BIGINT PRIMARY KEY,
    -- message, attachment, user, sticker, emoji or guild
    target_kind      TEXT NOT NULL,
    target_id        BIGINT NOT NULL,
    category         TEXT NOT NULL,
    -- 0 = child safety (first), 1 = serious, 2 = everything else
    priority         SMALLINT NOT NULL,
    -- open, review or closed
    status           TEXT NOT NULL DEFAULT 'open',
    -- No foreign keys: the evidence outlives deleted accounts, messages and outhouses.
    reported_user_id BIGINT,
    guild_id         BIGINT,
    channel_id       BIGINT,
    message_id       BIGINT,
    -- What it looked like when reported (and edits made afterwards).
    snapshot         JSONB NOT NULL,
    report_count     INT NOT NULL DEFAULT 1,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at        TIMESTAMPTZ,
    closed_by        BIGINT,
    resolution       TEXT,
    -- Child-safety evidence is kept after closing until the owner releases it.
    evidence_released BOOLEAN NOT NULL DEFAULT false
);
-- One open report per thing; more reports of it add to its count.
CREATE UNIQUE INDEX reports_open_target_idx ON reports(target_kind, target_id) WHERE status <> 'closed';
CREATE INDEX reports_inbox_idx ON reports(status, priority, created_at DESC);
CREATE INDEX reports_user_idx ON reports(reported_user_id);
CREATE INDEX reports_message_idx ON reports(message_id) WHERE message_id IS NOT NULL;

-- Who reported it (never shown to the person reported).
CREATE TABLE report_reporters (
    report_id   BIGINT NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
    reporter_id BIGINT NOT NULL,
    category    TEXT NOT NULL,
    details     TEXT NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (report_id, reporter_id)
);
CREATE INDEX report_reporters_recent_idx ON report_reporters(reporter_id, created_at);

-- Files a report refers to, with their SHA-256. Kept on disk while the
-- report is open (and child-safety evidence until released).
CREATE TABLE report_files (
    report_id    BIGINT NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
    storage_key  TEXT NOT NULL,
    filename     TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size         BIGINT NOT NULL DEFAULT 0,
    sha256       TEXT,
    PRIMARY KEY (report_id, storage_key)
);
CREATE INDEX report_files_key_idx ON report_files(storage_key);

-- Files nobody can download (451) pending review; still on disk.
CREATE TABLE quarantined_files (
    storage_key TEXT PRIMARY KEY,
    report_id   BIGINT REFERENCES reports(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The site-level safety log: every action on reports and accounts, and the
-- owner's private notes.
CREATE TABLE safety_log (
    id             BIGINT PRIMARY KEY,
    actor_id       BIGINT,
    action         TEXT NOT NULL,
    report_id      BIGINT,
    target_user_id BIGINT,
    details        TEXT NOT NULL DEFAULT '',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX safety_log_report_idx ON safety_log(report_id);
CREATE INDEX safety_log_user_idx ON safety_log(target_user_id);
