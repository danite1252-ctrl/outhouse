-- The local image detector: per-file scan results, the owner's settings
-- (site-wide and per outhouse) and detector reports. No picture, thumbnail
-- or copy is ever stored here: only the score, labels and what was decided.

-- ---- Settings ------------------------------------------------------------------

ALTER TABLE site_settings
    ADD COLUMN detector_enabled     BOOLEAN NOT NULL DEFAULT true,
    -- low / medium / high (thresholds in api::detector)
    ADD COLUMN detector_sensitivity TEXT NOT NULL DEFAULT 'medium'
        CHECK (detector_sensitivity IN ('low', 'medium', 'high')),
    -- Flagged pictures outside 18+ channels: hold for approval, or blur only.
    ADD COLUMN detector_action      TEXT NOT NULL DEFAULT 'hold'
        CHECK (detector_action IN ('hold', 'blur')),
    -- DMs and group DMs: hold pictures to and from teens until checked.
    ADD COLUMN detector_hold_teens  BOOLEAN NOT NULL DEFAULT true;

ALTER TABLE guilds
    ADD COLUMN detector_enabled     BOOLEAN NOT NULL DEFAULT true,
    -- NULL = the site-wide setting
    ADD COLUMN detector_sensitivity TEXT
        CHECK (detector_sensitivity IN ('low', 'medium', 'high')),
    ADD COLUMN detector_action      TEXT
        CHECK (detector_action IN ('hold', 'blur')),
    ADD COLUMN detector_hold_teens  BOOLEAN NOT NULL DEFAULT true;

-- ---- Scan results (one row per stored file) ----------------------------------

CREATE TABLE file_scans (
    -- Detector reports point at this (reports.target_kind = 'file').
    id              BIGINT NOT NULL UNIQUE,
    storage_key     TEXT PRIMARY KEY,
    -- attachment, avatar, banner, emoji or sticker
    kind            TEXT NOT NULL,
    content_type    TEXT NOT NULL,
    uploader_id     BIGINT,
    guild_id        BIGINT,
    channel_id      BIGINT,
    message_id      BIGINT,
    -- Sent to or from a teen (a DM with a teen, a channel a teen can see,
    -- or a teen's own upload).
    teen            BOOLEAN NOT NULL DEFAULT false,
    teen_author     BOOLEAN NOT NULL DEFAULT false,
    -- An 18+ channel that only adults can see.
    adult_only      BOOLEAN NOT NULL DEFAULT false,
    status          TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'clean', 'flagged', 'error', 'skipped')),
    -- What people see: none, checking (pending), held (for review),
    -- blur (click through) or hidden (avatars, banners, emoji, stickers).
    hold            TEXT NOT NULL DEFAULT 'none'
        CHECK (hold IN ('none', 'checking', 'held', 'blur', 'hidden')),
    -- The owner's decision: approved (released) or blurred (kept blurred).
    review          TEXT CHECK (review IN ('approved', 'blurred')),
    -- false: scanned for a report's review screen only (nothing is held).
    apply_hold      BOOLEAN NOT NULL DEFAULT true,
    score           REAL,
    labels          JSONB NOT NULL DEFAULT '[]',
    model           TEXT,
    threshold       REAL,
    error           TEXT,
    attempts        INT NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    scanned_at      TIMESTAMPTZ
);
CREATE INDEX file_scans_queue_idx ON file_scans(next_attempt_at) WHERE status = 'pending';
CREATE INDEX file_scans_held_idx ON file_scans(hold) WHERE hold <> 'none';

-- ---- Reports ---------------------------------------------------------------------

-- 'user' (someone reported it) or 'detector' (the image detector flagged it).
ALTER TABLE reports ADD COLUMN source TEXT NOT NULL DEFAULT 'user';
