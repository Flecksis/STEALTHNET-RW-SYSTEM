CREATE TABLE promo_codes (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    code text NOT NULL CHECK (code ~ '^[A-Z0-9_-]{3,32}$'),
    kind text NOT NULL CHECK (kind IN ('percent', 'fixed', 'days')),
    percent integer CHECK (percent BETWEEN 1 AND 100),
    amount_minor bigint CHECK (amount_minor > 0),
    currency text CHECK (currency IN ('USD', 'RUB')),
    days integer CHECK (days BETWEEN 1 AND 3650),
    max_uses integer CHECK (max_uses > 0),
    per_client integer NOT NULL DEFAULT 1 CHECK (per_client > 0),
    valid_from timestamptz,
    valid_until timestamptz,
    first_purchase_only boolean NOT NULL DEFAULT false,
    tariff_ids bigint[] NOT NULL DEFAULT '{}',
    active boolean NOT NULL DEFAULT true,
    note text,
    created_by bigint REFERENCES admins (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    disabled_at timestamptz,
    CONSTRAINT promo_codes_code_key UNIQUE (code),
    CONSTRAINT promo_codes_value_check CHECK (
        (kind = 'percent' AND percent IS NOT NULL AND amount_minor IS NULL AND currency IS NULL AND days IS NULL)
        OR (kind = 'fixed' AND amount_minor IS NOT NULL AND currency IS NOT NULL AND percent IS NULL AND days IS NULL)
        OR (kind = 'days' AND days IS NOT NULL AND percent IS NULL AND amount_minor IS NULL AND currency IS NULL)
    ),
    CONSTRAINT promo_codes_dates_check CHECK (valid_from IS NULL OR valid_until IS NULL OR valid_until > valid_from)
);
CREATE INDEX promo_codes_list_idx ON promo_codes (id DESC);
CREATE TRIGGER promo_codes_touch BEFORE UPDATE ON promo_codes FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE promo_redemptions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    promo_id bigint NOT NULL REFERENCES promo_codes (id),
    client_id bigint NOT NULL REFERENCES clients (id),
    order_id bigint REFERENCES orders (id),
    subscription_id bigint REFERENCES subscriptions (id),
    status text NOT NULL CHECK (status IN ('reserved', 'used', 'released')),
    reserved_until timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    used_at timestamptz,
    released_at timestamptz,
    CONSTRAINT promo_redemptions_order_key UNIQUE (order_id)
);
CREATE INDEX promo_redemptions_promo_idx ON promo_redemptions (promo_id, status);
CREATE INDEX promo_redemptions_client_idx ON promo_redemptions (client_id, promo_id);
