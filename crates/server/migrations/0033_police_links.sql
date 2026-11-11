-- Evidence hand-over from the app: the site owner packs a report's evidence
-- (the original files byte for byte, the evidence package, HANDOVER.txt and
-- MANIFEST.sha256) into one sealed .tar.gz on the server and gives police a
-- one-time download link for it, so the files never need to touch the
-- owner's own device. Only the token's SHA-256 is kept (the link itself is
-- shown once); the package file is deleted from the server once the link is
-- used up, expires or is revoked. Rows stay as a record (no foreign keys,
-- like the rest of the safety tables).
CREATE TABLE police_links (
    id             BIGINT PRIMARY KEY,
    report_id      BIGINT NOT NULL,
    token_sha256   TEXT NOT NULL UNIQUE,
    -- The first characters of the token, for the log and the console.
    token_prefix   TEXT NOT NULL,
    package_name   TEXT NOT NULL,
    -- The package's file name in the packages folder; NULL once deleted.
    package_file   TEXT,
    package_sha256 TEXT NOT NULL,
    package_size   BIGINT NOT NULL,
    file_count     INT NOT NULL,
    reference      TEXT NOT NULL DEFAULT '',
    created_by     BIGINT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at     TIMESTAMPTZ NOT NULL,
    max_downloads  INT NOT NULL,
    downloads      INT NOT NULL DEFAULT 0,
    last_download_at TIMESTAMPTZ,
    revoked_at     TIMESTAMPTZ,
    revoked_by     BIGINT,
    deleted_at     TIMESTAMPTZ
);
CREATE INDEX police_links_report_idx ON police_links(report_id);
CREATE INDEX police_links_live_idx ON police_links(expires_at) WHERE package_file IS NOT NULL;
