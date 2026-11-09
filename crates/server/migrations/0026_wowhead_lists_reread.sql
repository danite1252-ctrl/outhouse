-- Loot lists are read differently now (Wowhead's real pages: redirects to
-- the page's full address, Season of Discovery rows left out, encounters
-- like Attumen and Midnight or the Opera event as one boss, chests and boss
-- pages read when the zone page doesn't say). Read every saved list again
-- the next time it's used; the saved items show until then.
UPDATE wowhead_lists SET ok_at = ok_at - interval '30 days' WHERE ok_at IS NOT NULL;
UPDATE wowhead_lists SET fetched_at = fetched_at - interval '30 days';
