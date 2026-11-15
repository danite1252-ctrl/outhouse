-- Raids from before attendance was merged (0047) that only have hand marks:
-- people who signed up but were never ticked stay uncounted, as they were
-- in the old system, rather than becoming no-shows. Raids from now on (and
-- any old raid that gets an MRT import) follow the merged rules.
ALTER TABLE events ADD COLUMN attendance_legacy BOOLEAN NOT NULL DEFAULT false;

UPDATE events e
   SET attendance_legacy = true
 WHERE e.starts_at < now()
   AND NOT EXISTS (SELECT 1 FROM attendance_pulls p WHERE p.event_id = e.id);

CREATE OR REPLACE VIEW raid_attendance_outcomes AS
WITH raids AS (
    SELECT e.id
    FROM events e
    WHERE EXISTS (SELECT 1 FROM attendance_pulls p WHERE p.event_id = e.id)
       OR EXISTS (SELECT 1 FROM event_attendance a WHERE a.event_id = e.id)
), people AS (
    SELECT m.event_id, m.user_id
    FROM attendance_marks m
    WHERE m.user_id IS NOT NULL AND m.present
    UNION
    SELECT a.event_id, a.user_id FROM event_attendance a
    UNION
    SELECT s.event_id, s.user_id
    FROM event_signups s
    WHERE s.status IN ('yes', 'late', 'tentative')
      AND s.event_id IN (SELECT id FROM raids)
)
SELECT p.event_id,
       p.user_id,
       a.status AS manual_status,
       s.status AS signup_status,
       EXISTS (SELECT 1 FROM attendance_marks m
               WHERE m.event_id = p.event_id AND m.user_id = p.user_id AND m.present)
           AS imported_present,
       CASE
           WHEN a.status IN ('present', 'late', 'benched') THEN 'attended'
           WHEN a.status = 'excused' THEN 'excused'
           WHEN a.status IS NULL AND EXISTS (
                   SELECT 1 FROM attendance_marks m
                   WHERE m.event_id = p.event_id AND m.user_id = p.user_id AND m.present)
               THEN 'attended'
           -- Old hand-only raids: never ticked means not counted.
           WHEN a.status IS NULL AND e.attendance_legacy
                AND NOT EXISTS (SELECT 1 FROM attendance_pulls pl WHERE pl.event_id = p.event_id)
               THEN 'unmarked'
           WHEN s.status IN ('yes', 'late', 'tentative') THEN 'no_show'
           ELSE 'absent'
       END AS outcome
FROM people p
JOIN events e ON e.id = p.event_id
LEFT JOIN event_attendance a ON a.event_id = p.event_id AND a.user_id = p.user_id
LEFT JOIN event_signups s ON s.event_id = p.event_id AND s.user_id = p.user_id;
