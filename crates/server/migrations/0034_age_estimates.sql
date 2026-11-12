-- Age estimates from the image detector (faces found and each face's
-- estimated age, a preliminary automated estimate, not evidence), and the
-- rule that escalates explicit pictures with a face estimated clearly young
-- to a child-safety report. Numbers only: no face crops, embeddings or
-- copies of anything are stored.

ALTER TABLE file_scans
    -- NULL: scanned before faces were checked (or the sidecar doesn't do them).
    ADD COLUMN face_count       INT,
    -- The youngest face's estimated age in years (NULL: no face).
    ADD COLUMN min_est_age      REAL,
    -- [{"age": years, "confidence": 0-1}] youngest first, at most 10.
    ADD COLUMN ages             JSONB NOT NULL DEFAULT '[]',
    ADD COLUMN age_model        TEXT,
    -- The age rule escalated this file to child safety…
    ADD COLUMN age_escalated_at TIMESTAMPTZ,
    -- …and the site owner dismissed that as a mistake (no more automatic
    -- escalations for this file).
    ADD COLUMN age_cleared_at   TIMESTAMPTZ;

ALTER TABLE site_settings
    -- Escalate explicit pictures with a young-looking face to child safety.
    ADD COLUMN detector_age_check       BOOLEAN NOT NULL DEFAULT true,
    -- The youngest face's estimate at or below this counts as young.
    ADD COLUMN detector_age_threshold   INT NOT NULL DEFAULT 13
        CHECK (detector_age_threshold IN (10, 13, 16)),
    -- The nudity score that counts as explicit for this rule (lower than
    -- the normal hold threshold: the combination is what matters).
    ADD COLUMN detector_escalation_score REAL NOT NULL DEFAULT 0.40
        CHECK (detector_escalation_score >= 0.05 AND detector_escalation_score <= 0.95);
