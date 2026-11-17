CREATE TABLE provider_refunds (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    payment_id bigint NOT NULL REFERENCES payments (id),
    amount_minor bigint NOT NULL CHECK (amount_minor > 0),
    currency text NOT NULL CHECK (currency IN ('USD', 'RUB', 'XTR')),
    subscription_action text NOT NULL CHECK (subscription_action IN ('keep', 'cut', 'disable')),
    reason text,
    admin_id bigint REFERENCES admins (id),
    provider_refund_id text,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'succeeded', 'declined')),
    refund_id bigint REFERENCES refunds (id),
    failure_reason text,
    created_at timestamptz NOT NULL DEFAULT now(),
    resolved_at timestamptz,
    CHECK ((status = 'pending') = (resolved_at IS NULL)),
    CHECK (status <> 'declined' OR refund_id IS NULL)
);
CREATE INDEX provider_refunds_payment_idx ON provider_refunds (payment_id, id);
CREATE INDEX provider_refunds_pending_idx ON provider_refunds (created_at) WHERE status = 'pending';
