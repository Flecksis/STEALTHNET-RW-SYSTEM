ALTER TABLE webhook_inbox DROP CONSTRAINT webhook_inbox_outcome_check;
ALTER TABLE webhook_inbox ADD CONSTRAINT webhook_inbox_outcome_check CHECK (outcome IN ('received', 'accepted', 'ignored', 'rejected_signature', 'rejected_payload', 'rejected_business', 'not_accepted', 'payment_not_found', 'amount_mismatch', 'error'));
UPDATE webhook_inbox SET outcome = 'rejected_business' WHERE outcome = 'rejected_payload' AND body IS NOT NULL;
DROP INDEX webhook_inbox_rejected_idx;
CREATE INDEX webhook_inbox_rejected_idx ON webhook_inbox (received_at) WHERE outcome = 'rejected_signature' OR body IS NULL;
