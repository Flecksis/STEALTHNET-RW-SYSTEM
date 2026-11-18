ALTER TABLE payment_notices
    ADD COLUMN resolved_at timestamptz,
    ADD COLUMN resolved_by bigint REFERENCES admins (id),
    ADD COLUMN resolution text;

CREATE INDEX payment_notices_waiting_idx ON payment_notices (created_at, id)
    WHERE resolved_at IS NULL AND kind IN ('manual_credit', 'refund_reported');
