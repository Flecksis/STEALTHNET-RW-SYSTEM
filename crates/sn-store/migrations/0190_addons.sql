ALTER TABLE tariffs ADD COLUMN traffic_addon jsonb;

CREATE TABLE subscription_extra_traffic (
    subscription_id bigint PRIMARY KEY REFERENCES subscriptions (id) ON DELETE CASCADE,
    bytes bigint NOT NULL CHECK (bytes > 0),
    window_started_at timestamptz,
    window_lifetime bigint CHECK (window_lifetime >= 0),
    window_ends_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX subscription_extra_traffic_due_idx
    ON subscription_extra_traffic (window_ends_at)
    WHERE window_ends_at IS NOT NULL;

CREATE TABLE extra_traffic_orders (
    order_id bigint PRIMARY KEY REFERENCES orders (id),
    subscription_id bigint NOT NULL REFERENCES subscriptions (id) ON DELETE CASCADE,
    bytes bigint NOT NULL CHECK (bytes > 0),
    limit_target bigint NOT NULL CHECK (limit_target > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    applied_at timestamptz
);

ALTER TABLE option_purchases
    ADD COLUMN item text NOT NULL DEFAULT 'devices' CHECK (item IN ('devices', 'traffic')),
    ADD COLUMN traffic_bytes bigint CHECK (traffic_bytes > 0),
    ADD COLUMN days integer CHECK (days > 0);

ALTER TABLE option_purchases DROP CONSTRAINT option_purchases_check;
ALTER TABLE option_purchases DROP CONSTRAINT option_purchases_months_check;

ALTER TABLE option_purchases
    ADD CONSTRAINT option_purchases_devices_check CHECK (item <> 'devices' OR devices_after > devices_before),
    ADD CONSTRAINT option_purchases_traffic_check CHECK (item <> 'traffic' OR traffic_bytes IS NOT NULL),
    ADD CONSTRAINT option_purchases_months_check CHECK (months >= 0);
