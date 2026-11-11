-- IP addresses, kept for safety (see crates/server/src/ips.rs): police in
-- child-safety cases, and a quiet hint about ban evasion. Only the site
-- owner can see them. Shared addresses are common (households, mobile
-- networks, VPNs), so a match is a hint, never proof: nothing is blocked
-- or banned automatically.

-- One row per (account, address), updated in place. `kind` is how the
-- address was first seen: register, login or session (signed-in activity,
-- at most once an hour per session). Rows not seen for IP_RETENTION_DAYS
-- (default 90) are deleted.
CREATE TABLE user_ips (
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    ip         INET NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('register', 'login', 'session')),
    first_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen  TIMESTAMPTZ NOT NULL DEFAULT now(),
    count      INT NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, ip)
);
CREATE INDEX user_ips_ip_idx ON user_ips(ip, last_seen);
CREATE INDEX user_ips_last_seen_idx ON user_ips(last_seen);

ALTER TABLE users
    ADD COLUMN last_ip    INET,
    ADD COLUMN last_ip_at TIMESTAMPTZ;

-- A report's copy of the reported person's addresses (made when the report
-- is made, when its evidence is exported, and before retention would delete
-- them while the report is open), so retention can keep running on
-- user_ips. Deleted with the report's other evidence rules (see ips.rs).
CREATE TABLE report_ips (
    report_id  BIGINT NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL,
    ip         INET NOT NULL,
    kind       TEXT NOT NULL,
    first_seen TIMESTAMPTZ NOT NULL,
    last_seen  TIMESTAMPTZ NOT NULL,
    count      INT NOT NULL DEFAULT 1,
    PRIMARY KEY (report_id, user_id, ip)
);
CREATE INDEX report_ips_last_seen_idx ON report_ips(last_seen);

-- Ban-evasion hints already raised: one per (account, matched account),
-- never repeated. No address is kept here.
CREATE TABLE ip_matches (
    user_id         BIGINT NOT NULL,
    matched_user_id BIGINT NOT NULL,
    report_id       BIGINT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, matched_user_id)
);
