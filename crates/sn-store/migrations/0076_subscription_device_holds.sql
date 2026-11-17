CREATE TABLE subscription_device_holds (
    subscription_id bigint PRIMARY KEY REFERENCES subscriptions (id) ON DELETE CASCADE,
    order_id bigint NOT NULL REFERENCES orders (id),
    held_limit integer NOT NULL CHECK (held_limit > 0),
    held_until timestamptz NOT NULL,
    limit_after integer NOT NULL CHECK (limit_after > 0),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX subscription_device_holds_due_idx ON subscription_device_holds (held_until);
