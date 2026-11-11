-- "Suspend now" on child-safety reports: the site owner suspends the
-- reported account until they've reviewed it. The person only ever sees the
-- neutral "suspended pending review of a report" (never the category or who
-- reported them). When and by whom any suspension was made is kept so the
-- console can say "Suspended at <time> by you".
ALTER TABLE users
    ADD COLUMN suspended_at      TIMESTAMPTZ,
    ADD COLUMN suspended_by      BIGINT,
    -- Suspended until the owner has reviewed it (shown as "pending review",
    -- not "permanently"); stored with suspended_until = 9999-12-31.
    ADD COLUMN suspension_pending BOOLEAN NOT NULL DEFAULT false;

-- ---- Blocked networks -----------------------------------------------------
-- The site owner can block a permanently suspended person's IP addresses
-- (IPv4: that address; IPv6: its /64). While a block is active nobody can
-- sign up, sign in, use the API or connect from it (strict, the owner's
-- choice), except the site owner and accounts he exempts. Rows are a
-- deliberate safety record: kept for their own lifetime (removing a block
-- marks it removed), whatever the IP retention.
CREATE TABLE ip_blocks (
    id             BIGINT PRIMARY KEY,
    ip             CIDR NOT NULL,
    source_user_id BIGINT,
    report_id      BIGINT,
    reason         TEXT NOT NULL DEFAULT '',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- NULL = until the owner removes it.
    expires_at     TIMESTAMPTZ,
    created_by     BIGINT,
    removed_at     TIMESTAMPTZ,
    removed_by     BIGINT
);
CREATE UNIQUE INDEX ip_blocks_active_idx ON ip_blocks(ip) WHERE removed_at IS NULL;
CREATE INDEX ip_blocks_user_idx ON ip_blocks(source_user_id);

-- Every refused attempt from a blocked network (kept like evidence: a year,
-- or while the blocked person's child-safety evidence is kept).
CREATE TABLE blocked_attempts (
    id         BIGINT PRIMARY KEY,
    block_id   BIGINT NOT NULL REFERENCES ip_blocks(id) ON DELETE CASCADE,
    ip         INET NOT NULL,
    -- register, login, request or gateway
    kind       TEXT NOT NULL,
    user_id    BIGINT,
    username   TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX blocked_attempts_block_idx ON blocked_attempts(block_id, created_at DESC);
CREATE INDEX blocked_attempts_user_idx ON blocked_attempts(user_id);

-- "Allow from blocked networks": the owner's exemption for someone he
-- knows is innocent (e.g. family on the same connection).
ALTER TABLE users ADD COLUMN ip_block_exempt BOOLEAN NOT NULL DEFAULT false;
