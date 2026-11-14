-- Realm time per game (Outhouse settings → Raids → Realm time).
-- realm_time_zone / realm_region (0040) stay as they are and become the
-- "all games" default, so nothing set before is lost. realm_games holds each
-- game's own realm on top of that:
-- {"forever": {"time_zone": "America/North_Dakota/Center", "region": "us"},
--  "tbc": {"time_zone": "America/New_York", "region": ""}}
-- ("" = use the default). Only games with something set are kept.
ALTER TABLE raid_settings
    ADD COLUMN realm_games JSONB NOT NULL DEFAULT '{}'::jsonb;
