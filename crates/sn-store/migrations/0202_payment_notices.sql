CREATE TABLE payment_notices (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    provider text NOT NULL,
    event_key text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('dispute', 'chargeback', 'unmatched_refund', 'refund_reported', 'aml_check', 'manual_credit')),
    payment_id bigint REFERENCES payments (id),
    client_id bigint REFERENCES clients (id),
    amount_minor bigint,
    currency text CHECK (currency IN ('USD', 'RUB', 'XTR')),
    text text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT payment_notices_event_key UNIQUE (provider, event_key),
    CHECK ((amount_minor IS NULL) = (currency IS NULL))
);
CREATE INDEX payment_notices_payment_idx ON payment_notices (payment_id) WHERE payment_id IS NOT NULL;

ALTER TABLE refunds ADD COLUMN request_key uuid;
CREATE UNIQUE INDEX refunds_request_key ON refunds (request_key) WHERE request_key IS NOT NULL;

ALTER TABLE provider_refunds ADD COLUMN request_key uuid;
CREATE UNIQUE INDEX provider_refunds_request_key ON provider_refunds (request_key) WHERE request_key IS NOT NULL;
