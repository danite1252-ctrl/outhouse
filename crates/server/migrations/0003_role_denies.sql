-- Roles can now deny permissions as well as allow them.
-- A bit in neither column means "inherit" (from @everyone or lower roles).
ALTER TABLE roles ADD COLUMN deny_permissions BIGINT NOT NULL DEFAULT 0;
