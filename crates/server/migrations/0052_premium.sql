-- Premium: "Golden Throne" (per person) and "Golden Outhouse" (per outhouse).
-- No tiers: premium or not. Entitlements come from a grant by the site owner
-- (or staff with `grant_premium`) now, and from Stripe later; both just add
-- rows here. Something is premium while it has an active row:
--   revoked_at IS NULL AND starts_at <= now() AND (ends_at IS NULL OR ends_at > now())
-- Every check goes through oh_user_premium / oh_guild_premium (and the Rust
-- helpers in api/premium.rs that call them).

CREATE TABLE user_premium (
    id          BIGINT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 'grant' (site owner / staff) or 'stripe' (later)
    source      TEXT NOT NULL CHECK (source IN ('grant', 'stripe')),
    starts_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- NULL = forever
    ends_at     TIMESTAMPTZ,
    granted_by  BIGINT REFERENCES users(id) ON DELETE SET NULL,
    note        TEXT NOT NULL DEFAULT '',
    -- Stripe's subscription id later (NULL for grants).
    external_ref TEXT,
    revoked_at  TIMESTAMPTZ,
    revoked_by  BIGINT REFERENCES users(id) ON DELETE SET NULL,
    -- Set by the background task once the end has been logged and announced.
    ended_logged BOOLEAN NOT NULL DEFAULT false,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX user_premium_user_idx ON user_premium (user_id);

CREATE TABLE outhouse_premium (
    id              BIGINT PRIMARY KEY,
    guild_id        BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    -- 'grant' or 'stripe' (any member can sponsor, later). Free slots from
    -- Golden Throne members live in premium_slots (shown as source 'slot').
    source          TEXT NOT NULL CHECK (source IN ('grant', 'stripe')),
    sponsor_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    starts_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    ends_at         TIMESTAMPTZ,
    granted_by      BIGINT REFERENCES users(id) ON DELETE SET NULL,
    note            TEXT NOT NULL DEFAULT '',
    external_ref    TEXT,
    revoked_at      TIMESTAMPTZ,
    revoked_by      BIGINT REFERENCES users(id) ON DELETE SET NULL,
    ended_logged    BOOLEAN NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX outhouse_premium_guild_idx ON outhouse_premium (guild_id);

-- Each Golden Throne member's one free Golden Outhouse slot. It counts only
-- while they have Golden Throne and are a member of that outhouse; moving it
-- to a different outhouse is allowed once every 7 days (applied_at).
CREATE TABLE premium_slots (
    user_id     BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    guild_id    BIGINT REFERENCES guilds(id) ON DELETE SET NULL,
    applied_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX premium_slots_guild_idx ON premium_slots (guild_id);

CREATE OR REPLACE FUNCTION oh_user_premium(uid BIGINT) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM user_premium p
        WHERE p.user_id = uid AND p.revoked_at IS NULL
          AND p.starts_at <= now() AND (p.ends_at IS NULL OR p.ends_at > now()))
$$;

CREATE OR REPLACE FUNCTION oh_guild_premium(gid BIGINT) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM outhouse_premium p
        WHERE p.guild_id = gid AND p.revoked_at IS NULL
          AND p.starts_at <= now() AND (p.ends_at IS NULL OR p.ends_at > now()))
    OR EXISTS (
        SELECT 1 FROM premium_slots s
        JOIN guild_members m ON m.guild_id = s.guild_id AND m.user_id = s.user_id
        WHERE s.guild_id = gid AND oh_user_premium(s.user_id))
$$;

-- The owner's switches (one row). NULL = use the environment's default
-- (MESSAGE_CHARS, PREMIUM_MESSAGE_CHARS, PREMIUM_EMOJI, PREMIUM_ANIMATED);
-- the server writes the values in effect into the *_now columns at start-up
-- and whenever the owner changes one, so SQL (oh_file_url) can read them.
CREATE TABLE premium_settings (
    id                    SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    free_message_chars    INT,
    premium_message_chars INT,
    -- true: other outhouses' emoji need Golden Throne; false: free for everyone
    emoji_premium         BOOLEAN,
    -- true: animated avatars/banners/outhouse pictures need premium
    -- (others see the first frame); false: free for everyone
    animated_premium      BOOLEAN,
    animated_premium_now  BOOLEAN NOT NULL DEFAULT false,
    updated_by            BIGINT REFERENCES users(id) ON DELETE SET NULL,
    updated_at            TIMESTAMPTZ
);
INSERT INTO premium_settings (id) VALUES (1);

-- ---- Golden Outhouse: vanity invite links ------------------------------------
-- /invite/<vanity_code>: lower-case a-z, 0-9 and hyphens, unique. When the
-- outhouse's premium lapses the link pauses (vanity_paused_at) and the name
-- stays held for this outhouse for 30 days, then it's released.
ALTER TABLE guilds
    ADD COLUMN vanity_code      TEXT,
    ADD COLUMN vanity_uses      INT NOT NULL DEFAULT 0,
    ADD COLUMN vanity_set_at    TIMESTAMPTZ,
    ADD COLUMN vanity_paused_at TIMESTAMPTZ,
    -- Golden Throne members' personal soundboard sounds can be played here.
    ADD COLUMN allow_personal_sounds BOOLEAN NOT NULL DEFAULT true;
CREATE UNIQUE INDEX guilds_vanity_idx ON guilds (vanity_code) WHERE vanity_code IS NOT NULL;

-- ---- Golden Throne: personal sounds, themes, profile colours ------------------

CREATE TABLE user_sounds (
    id           BIGINT PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    emoji        TEXT NOT NULL DEFAULT '',
    storage_key  TEXT NOT NULL UNIQUE,
    content_type TEXT NOT NULL,
    size         BIGINT NOT NULL,
    volume       REAL NOT NULL DEFAULT 1.0,
    duration_ms  INT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX user_sounds_user_idx ON user_sounds (user_id);

-- One of your personal sounds as your entrance sound everywhere (overrides
-- the free entrance sound while you have Golden Throne).
CREATE TABLE premium_entrance (
    user_id  BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    sound_id BIGINT NOT NULL REFERENCES user_sounds(id) ON DELETE CASCADE
);

-- The app's colour theme ('ember' default; 'contrast' is free for everyone).
ALTER TABLE users
    ADD COLUMN app_theme TEXT NOT NULL DEFAULT 'ember',
    -- Two colours (0xRRGGBB) for a gradient on the profile card and page.
    ADD COLUMN profile_gradient INT[];

-- ---- Animated pictures ---------------------------------------------------------
-- Which outhouse a picture belongs to (icons and banners) and whether it
-- moves (GIF/WebP/APNG with more than one frame; NULL = not checked yet).
ALTER TABLE profile_files
    ADD COLUMN guild_id BIGINT,
    ADD COLUMN animated BOOLEAN;
UPDATE profile_files f SET guild_id = g.id
  FROM guilds g WHERE g.icon_key = f.storage_key OR g.banner_key = f.storage_key;
UPDATE profile_files SET animated = false
 WHERE content_type NOT IN ('image/gif', 'image/webp', 'image/png');

-- Picture links say whether the picture moves (`anim=1`; the web client can
-- show the first frame until hovered) or, when animated pictures are a
-- premium perk and the owner hasn't got it, that only the first frame is
-- shown (`still=1`; /files serves the first frame). The `v=` part (the
-- detector's state) is as before.
CREATE OR REPLACE FUNCTION oh_file_url(key TEXT, name TEXT) RETURNS TEXT
LANGUAGE sql STABLE AS $$
    SELECT CASE WHEN key IS NULL THEN NULL ELSE
        '/files/' || key || '/' || name || COALESCE('?' || NULLIF(concat_ws('&',
            (SELECT 'v=' || left(md5(s.status || ':' || s.hold || ':' || COALESCE(s.review, '')), 8)
               FROM file_scans s WHERE s.storage_key = key),
            (SELECT CASE
                      WHEN (SELECT animated_premium_now FROM premium_settings WHERE id = 1)
                           AND NOT (CASE WHEN f.guild_id IS NOT NULL THEN oh_guild_premium(f.guild_id)
                                         ELSE oh_user_premium(f.user_id) END)
                      THEN 'still=1' ELSE 'anim=1' END
               FROM profile_files f WHERE f.storage_key = key AND f.animated)
        ), ''), '')
    END
$$;
