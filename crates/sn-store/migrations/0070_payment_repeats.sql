CREATE TABLE payment_repeats (
    payment_id bigint PRIMARY KEY REFERENCES payments (id),
    original_payment_id bigint REFERENCES payments (id),
    action text NOT NULL CHECK (action IN ('balance', 'review')),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX payment_repeats_original_idx ON payment_repeats (original_payment_id) WHERE original_payment_id IS NOT NULL;
