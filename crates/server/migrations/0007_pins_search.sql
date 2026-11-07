-- Pinned messages and message search.

ALTER TABLE messages ADD COLUMN pinned_at TIMESTAMPTZ;
ALTER TABLE messages ADD COLUMN pinned_by BIGINT REFERENCES users(id) ON DELETE SET NULL;
CREATE INDEX messages_pinned_idx ON messages(channel_id, pinned_at DESC) WHERE pinned_at IS NOT NULL;

-- Full-text search ('simple' = no stemming, works for any language and for names).
CREATE INDEX messages_search_idx ON messages USING GIN (to_tsvector('simple', content));
CREATE INDEX messages_author_idx ON messages(author_id, id DESC);
