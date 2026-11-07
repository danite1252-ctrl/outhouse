-- Raid planner extras: repeating raids, sign-up deadlines, auto-bench and
-- attendance.

-- A repeating raid. The server makes the actual raids (rows in `events`) a
-- few weeks ahead, so each one has its own sign-ups.
CREATE TABLE event_series (
    id                      BIGINT PRIMARY KEY,
    guild_id                BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    channel_id              BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    creator_id              BIGINT NOT NULL REFERENCES users(id),
    title                   TEXT NOT NULL,
    description             TEXT NOT NULL DEFAULT '',
    duration_minutes        INT NOT NULL,
    max_tanks               INT NOT NULL,
    max_healers             INT NOT NULL,
    max_dps                 INT NOT NULL,
    signup_deadline_minutes INT,
    -- IANA zone the start time is kept in, so daylight saving doesn't shift it.
    time_zone               TEXT NOT NULL,
    start_date              DATE NOT NULL,
    local_time              TIME NOT NULL,
    -- Bit 0 = Monday … bit 6 = Sunday.
    weekdays                INT NOT NULL,
    every_weeks             INT NOT NULL DEFAULT 1,
    until_date              DATE,
    max_count               INT,
    -- How many raids have been made so far, and up to which day.
    made_count              INT NOT NULL DEFAULT 0,
    generated_through       DATE,
    ended                   BOOLEAN NOT NULL DEFAULT false,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX event_series_active_idx ON event_series(generated_through) WHERE NOT ended;

-- Raids in a series get their chat card a week before they start.
ALTER TABLE events ALTER COLUMN message_id DROP NOT NULL;
ALTER TABLE events
    ADD COLUMN series_id BIGINT REFERENCES event_series(id) ON DELETE SET NULL,
    ADD COLUMN signup_deadline_minutes INT,
    ADD COLUMN attendance_taken BOOLEAN NOT NULL DEFAULT false;
CREATE INDEX events_series_idx ON events(series_id, starts_at);
CREATE INDEX events_unposted_idx ON events(starts_at) WHERE message_id IS NULL;

-- Set when someone was benched because their role was full: the status they
-- asked for. NULL for everyone else (including people benched by hand).
ALTER TABLE event_signups ADD COLUMN bench_wanted TEXT;

CREATE TABLE event_attendance (
    event_id  BIGINT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    user_id   BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 'present', 'late', 'absent' or 'benched'
    status    TEXT NOT NULL,
    marked_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
    marked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (event_id, user_id)
);
CREATE INDEX event_attendance_user_idx ON event_attendance(user_id);
