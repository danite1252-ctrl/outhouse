-- Keep channels.last_message_id pointing at the newest message people can see.
-- Before this, deleting the newest message (by hand, by the raid sign-up
-- clean-up, or by a safety hide) left the channel pointing at a message that
-- no longer existed, so it showed as unread forever and opening it couldn't
-- clear it.

CREATE OR REPLACE FUNCTION refresh_channel_last_message() RETURNS trigger AS $$
BEGIN
    IF OLD.thread_id IS NULL THEN
        UPDATE channels c
           SET last_message_id = (
                   SELECT max(m.id) FROM messages m
                    WHERE m.channel_id = OLD.channel_id
                      AND m.thread_id IS NULL
                      AND NOT m.safety_hidden)
         WHERE c.id = OLD.channel_id
           AND c.last_message_id = OLD.id;
    END IF;
    RETURN NULL;
END
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION restore_channel_last_message() RETURNS trigger AS $$
BEGIN
    IF NEW.thread_id IS NULL THEN
        UPDATE channels c
           SET last_message_id = NEW.id
         WHERE c.id = NEW.channel_id
           AND (c.last_message_id IS NULL OR c.last_message_id < NEW.id);
    END IF;
    RETURN NULL;
END
$$ LANGUAGE plpgsql;

CREATE TRIGGER messages_refresh_last_on_delete
    AFTER DELETE ON messages
    FOR EACH ROW EXECUTE FUNCTION refresh_channel_last_message();

CREATE TRIGGER messages_refresh_last_on_hide
    AFTER UPDATE OF safety_hidden ON messages
    FOR EACH ROW WHEN (NEW.safety_hidden AND NOT OLD.safety_hidden)
    EXECUTE FUNCTION refresh_channel_last_message();

CREATE TRIGGER messages_restore_last_on_unhide
    AFTER UPDATE OF safety_hidden ON messages
    FOR EACH ROW WHEN (OLD.safety_hidden AND NOT NEW.safety_hidden)
    EXECUTE FUNCTION restore_channel_last_message();

-- Repair channels that already point at a deleted or hidden message.
UPDATE channels c
   SET last_message_id = (
           SELECT max(m.id) FROM messages m
            WHERE m.channel_id = c.id
              AND m.thread_id IS NULL
              AND NOT m.safety_hidden)
 WHERE c.last_message_id IS NOT NULL
   AND NOT EXISTS (
           SELECT 1 FROM messages m
            WHERE m.id = c.last_message_id
              AND NOT m.safety_hidden);
