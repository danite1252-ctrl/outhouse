-- One raid attendance: the marks raid leaders set by hand ("Take
-- attendance", table event_attendance) and the attendance imported from
-- Method Raid Tools (0046) are now read together, everywhere.
--
-- A raid has attendance when it has imported boss pulls OR anyone marked by
-- hand. A hand mark is per member and overrides what the import says about
-- them on that raid (someone who joined after the last boss can be marked
-- present; someone MRT listed can be excused). Hand marks stay in
-- event_attendance, apart from the imports, so undoing or deleting an import
-- never takes them out, and each member has at most one mark per raid, so
-- nothing is counted twice.
--
-- Marks: present, late, benched (all count as attended), excused (new:
-- neither attended nor a no-show) and absent (a no-show if they signed up
-- yes, late or tentative; otherwise not counted).

-- Existing marks keep their meaning; tidy the spelling and drop anything the
-- old checks would never have let in.
UPDATE event_attendance SET status = lower(btrim(status))
    WHERE status <> lower(btrim(status));
DELETE FROM event_attendance
    WHERE status NOT IN ('present', 'late', 'absent', 'benched', 'excused');
ALTER TABLE event_attendance
    ADD CONSTRAINT event_attendance_status_check
    CHECK (status IN ('present', 'late', 'absent', 'benched', 'excused'));

-- The "attendance taken" flag on raids follows the hand marks.
UPDATE events e SET attendance_taken =
    EXISTS (SELECT 1 FROM event_attendance a WHERE a.event_id = e.id)
    WHERE attendance_taken <> EXISTS (SELECT 1 FROM event_attendance a WHERE a.event_id = e.id);

-- Per raid with attendance and member, what counts:
--   'attended' : marked present / late / benched, or (not marked) there for
--                at least one imported boss;
--   'excused'  : marked excused;
--   'no_show'  : signed up yes / late / tentative, and marked absent or (not
--                marked) not there for any imported boss;
--   'absent'   : marked absent without such a sign-up (not counted).
-- Characters MRT couldn't match to a member aren't here (they have no
-- member); the per-character numbers add them from attendance_marks.
CREATE VIEW raid_attendance_outcomes AS
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
           WHEN s.status IN ('yes', 'late', 'tentative') THEN 'no_show'
           ELSE 'absent'
       END AS outcome
FROM people p
LEFT JOIN event_attendance a ON a.event_id = p.event_id AND a.user_id = p.user_id
LEFT JOIN event_signups s ON s.event_id = p.event_id AND s.user_id = p.user_id;
