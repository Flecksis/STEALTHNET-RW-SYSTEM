ALTER TABLE provider_refunds ADD COLUMN polled_at timestamptz;
DROP INDEX provider_refunds_pending_idx;
CREATE INDEX provider_refunds_poll_idx ON provider_refunds (polled_at NULLS FIRST, created_at, id) WHERE status = 'pending';
