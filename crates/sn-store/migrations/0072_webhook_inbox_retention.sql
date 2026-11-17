CREATE INDEX webhook_inbox_rejected_idx ON webhook_inbox (received_at) WHERE outcome IN ('rejected_signature', 'rejected_payload') OR body IS NULL;
