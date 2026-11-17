CREATE TABLE option_purchases (
    order_id bigint PRIMARY KEY REFERENCES orders (id),
    subscription_id bigint NOT NULL REFERENCES subscriptions (id),
    client_id bigint NOT NULL REFERENCES clients (id),
    devices_before integer NOT NULL CHECK (devices_before >= 0),
    devices_after integer NOT NULL CHECK (devices_after > devices_before),
    months integer NOT NULL CHECK (months > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    applied_at timestamptz
);
CREATE INDEX option_purchases_subscription_idx ON option_purchases (subscription_id, created_at DESC);
