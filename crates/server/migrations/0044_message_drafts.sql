-- Files added to a message box before it's sent ("drafts"): uploaded as
-- soon as they're picked, pasted or dropped, so the image detector can
-- check them before anyone sees them. A draft belongs to one person, one
-- channel (and thread), and becomes a normal attachment when the message
-- is sent. Unsent drafts go after an hour (api::drafts), unless the file is
-- kept as evidence (a child-safety report).
--
-- Their detector scans are file_scans rows with kind = 'draft' (hold stays
-- 'none': nobody but the uploader has them, and they're never served).
CREATE TABLE message_drafts (
    id           BIGINT PRIMARY KEY,
    user_id      BIGINT NOT NULL,
    channel_id   BIGINT NOT NULL,
    thread_id    BIGINT,
    storage_key  TEXT NOT NULL UNIQUE,
    filename     TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size         BIGINT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL DEFAULT now() + INTERVAL '1 hour'
);
CREATE INDEX message_drafts_user_idx ON message_drafts(user_id);
CREATE INDEX message_drafts_expiry_idx ON message_drafts(expires_at);
-- Leftover draft scans (the draft was removed or expired) are tidied up by kind.
CREATE INDEX file_scans_draft_idx ON file_scans(kind) WHERE kind = 'draft';
