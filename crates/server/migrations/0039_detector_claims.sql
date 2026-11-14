-- Several detector workers (DETECTOR_WORKERS): a queue row is claimed by
-- one worker while it's being scanned. The claim is a lease: if the server
-- dies mid-scan it expires (and the server clears every claim when it
-- starts), so the picture is picked up again.
ALTER TABLE file_scans ADD COLUMN claimed_at TIMESTAMPTZ;
