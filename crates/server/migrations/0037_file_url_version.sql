-- Profile-style pictures (avatars, banners, outhouse icons) keep the same
-- link whether the image detector is holding them (placeholder) or not, so a
-- browser that already drew the placeholder kept showing it after the owner
-- approved the picture. The link now carries a short version that changes
-- with the scan's state, so approving (or holding) loads the right image.
CREATE OR REPLACE FUNCTION oh_file_url(key TEXT, name TEXT) RETURNS TEXT
LANGUAGE sql STABLE AS $$
    SELECT CASE WHEN key IS NULL THEN NULL ELSE
        '/files/' || key || '/' || name || COALESCE(
            (SELECT '?v=' || left(md5(s.status || ':' || s.hold || ':' || COALESCE(s.review, '')), 8)
               FROM file_scans s WHERE s.storage_key = key),
            '')
    END
$$;
