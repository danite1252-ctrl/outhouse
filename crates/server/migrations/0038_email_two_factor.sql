-- Email addresses (password resets by email link) and two-factor sign-in
-- with authenticator-app codes (TOTP).

-- Stored lower-cased. Two unconfirmed accounts may name the same address;
-- confirming it makes it that account's alone (the unique index below).
ALTER TABLE users
    ADD COLUMN email             TEXT,
    ADD COLUMN email_verified_at TIMESTAMPTZ,
    -- A new address waiting to be confirmed while the confirmed one stays
    -- in use (changing a confirmed email).
    ADD COLUMN email_pending     TEXT,
    -- The authenticator-app secret, encrypted with the server's
    -- AUTH_SECRET_KEY (twofa.rs). NULL = two-factor is off.
    ADD COLUMN totp_secret       TEXT,
    ADD COLUMN totp_enabled_at   TIMESTAMPTZ,
    -- A secret being set up (shown as a QR code, not confirmed yet).
    ADD COLUMN totp_pending      TEXT,
    -- The last 30-second step a code was accepted for (no code works twice).
    ADD COLUMN totp_last_step    BIGINT NOT NULL DEFAULT 0;
CREATE UNIQUE INDEX users_verified_email_idx ON users(email) WHERE email_verified_at IS NOT NULL;
CREATE INDEX users_email_idx ON users(email);

-- One-time links sent by email (or made by the site owner): confirming an
-- address, or resetting a password. Only a SHA-256 of the token is kept.
CREATE TABLE email_tokens (
    token_hash TEXT PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    purpose    TEXT NOT NULL CHECK (purpose IN ('verify', 'reset')),
    -- The address a confirmation link is for.
    email      TEXT,
    -- The site owner, for a reset link made in People.
    created_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at    TIMESTAMPTZ
);
CREATE INDEX email_tokens_user_idx ON email_tokens(user_id, purpose);

-- Two-factor recovery codes: SHA-256 hashes, each works once.
CREATE TABLE recovery_codes (
    user_id   BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL,
    used_at   TIMESTAMPTZ,
    PRIMARY KEY (user_id, code_hash)
);

-- "Trust this device for N days": a cookie (only its hash here) that skips
-- the code step for that account on that device.
CREATE TABLE trusted_devices (
    id           BIGINT PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash   TEXT NOT NULL UNIQUE,
    label        TEXT NOT NULL DEFAULT '',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL
);
CREATE INDEX trusted_devices_user_idx ON trusted_devices(user_id);

-- Password right, code still to come: a short-lived ticket, not a session.
CREATE TABLE pending_logins (
    token_hash TEXT PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    attempts   INT NOT NULL DEFAULT 0
);
CREATE INDEX pending_logins_user_idx ON pending_logins(user_id);
