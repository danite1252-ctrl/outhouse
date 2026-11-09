-- Soft reserves and the loot log (raid composition needs nothing stored).

-- Per raid (and per repeating series, which its new raids copy):
-- reserves each (0 = off), when they lock, and which raid's loot list.
ALTER TABLE events
    ADD COLUMN soft_reserves INT NOT NULL DEFAULT 0,
    ADD COLUMN reserve_lock TEXT NOT NULL DEFAULT 'deadline',
    -- A raid leader locked (true) or unlocked (false) reserves by hand.
    ADD COLUMN reserve_lock_override BOOLEAN,
    ADD COLUMN loot_instance TEXT NOT NULL DEFAULT '';
ALTER TABLE event_series
    ADD COLUMN soft_reserves INT NOT NULL DEFAULT 0,
    ADD COLUMN reserve_lock TEXT NOT NULL DEFAULT 'deadline',
    ADD COLUMN loot_instance TEXT NOT NULL DEFAULT '';

-- Raids a series makes take its loot settings, and a series split off
-- another ("Edit this and future raids" with new days) starts with the old
-- one's. Done here so every code path that makes them gets it.
CREATE FUNCTION events_take_series_loot() RETURNS trigger AS $$
DECLARE
    s RECORD;
BEGIN
    IF NEW.series_id IS NOT NULL THEN
        SELECT soft_reserves, reserve_lock, loot_instance INTO s
        FROM event_series WHERE id = NEW.series_id;
        IF FOUND THEN
            NEW.soft_reserves := s.soft_reserves;
            NEW.reserve_lock := s.reserve_lock;
            NEW.loot_instance := s.loot_instance;
        END IF;
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;
CREATE TRIGGER events_take_series_loot BEFORE INSERT ON events
    FOR EACH ROW EXECUTE FUNCTION events_take_series_loot();

CREATE FUNCTION series_take_split_loot() RETURNS trigger AS $$
DECLARE
    s RECORD;
BEGIN
    IF NEW.split_from IS NOT NULL THEN
        SELECT soft_reserves, reserve_lock, loot_instance INTO s
        FROM event_series WHERE id = NEW.split_from;
        IF FOUND THEN
            NEW.soft_reserves := s.soft_reserves;
            NEW.reserve_lock := s.reserve_lock;
            NEW.loot_instance := s.loot_instance;
        END IF;
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;
CREATE TRIGGER series_take_split_loot BEFORE INSERT ON event_series
    FOR EACH ROW EXECUTE FUNCTION series_take_split_loot();

-- Soft reserves: one row per person per item.
CREATE TABLE loot_reserves (
    event_id     BIGINT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id      INT NOT NULL,
    item_name    TEXT NOT NULL,
    item_quality INT NOT NULL DEFAULT 4,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (event_id, user_id, item_id)
);
CREATE INDEX loot_reserves_item_idx ON loot_reserves(event_id, item_id);

-- Items raid leaders added to a raid's loot list (on top of the built-in
-- few), per outhouse. `instance` is a listed raid's key or a lower-case name.
CREATE TABLE loot_items (
    guild_id   BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    instance   TEXT NOT NULL,
    item_id    INT NOT NULL,
    name       TEXT NOT NULL,
    quality    INT NOT NULL DEFAULT 4,
    boss       TEXT NOT NULL DEFAULT '',
    added_by   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, instance, item_id)
);

-- The loot log: who won what, in which raid. Raid details are copied so the
-- log survives the raid being deleted.
CREATE TABLE loot_awards (
    id             BIGINT PRIMARY KEY,
    guild_id       BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    event_id       BIGINT REFERENCES events(id) ON DELETE SET NULL,
    channel_id     BIGINT NOT NULL,
    raid_title     TEXT NOT NULL,
    raid_starts_at TIMESTAMPTZ NOT NULL,
    loot_instance  TEXT NOT NULL DEFAULT '',
    user_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    character_name TEXT NOT NULL DEFAULT '',
    item_id        INT NOT NULL,
    item_name      TEXT NOT NULL,
    item_quality   INT NOT NULL DEFAULT 4,
    reserved       BOOLEAN NOT NULL DEFAULT false,
    note           TEXT NOT NULL DEFAULT '',
    awarded_by     BIGINT REFERENCES users(id) ON DELETE SET NULL,
    awarded_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX loot_awards_guild_idx ON loot_awards(guild_id, awarded_at DESC);
CREATE INDEX loot_awards_user_idx ON loot_awards(guild_id, user_id, awarded_at DESC);
CREATE INDEX loot_awards_event_idx ON loot_awards(event_id);
