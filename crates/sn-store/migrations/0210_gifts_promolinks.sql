ALTER TABLE orders DROP CONSTRAINT orders_kind_check;
ALTER TABLE orders ADD CONSTRAINT orders_kind_check
    CHECK (kind IN ('new', 'renewal', 'change', 'topup', 'trial', 'module', 'admin_grant', 'module_grant'));

CREATE TABLE gifts (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id uuid NOT NULL DEFAULT gen_random_uuid(),
    code text NOT NULL CHECK (code ~ '^[A-Z0-9]{4}-[A-Z0-9]{4}-[A-Z0-9]{4}$'),
    order_id bigint NOT NULL REFERENCES orders (id),
    giver_id bigint NOT NULL REFERENCES clients (id),
    tariff_id bigint NOT NULL REFERENCES tariffs (id),
    period_id bigint NOT NULL,
    days integer NOT NULL CHECK (days BETWEEN 1 AND 3650),
    tariff_name text NOT NULL,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'redeemed', 'canceled')),
    expires_at timestamptz,
    redeemed_by bigint REFERENCES clients (id),
    redeemed_at timestamptz,
    grant_order_id bigint REFERENCES orders (id),
    canceled_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT gifts_public_id_key UNIQUE (public_id),
    CONSTRAINT gifts_code_key UNIQUE (code),
    CONSTRAINT gifts_order_key UNIQUE (order_id),
    CONSTRAINT gifts_redeemed_check CHECK (
        (status = 'redeemed') = (redeemed_by IS NOT NULL AND redeemed_at IS NOT NULL AND grant_order_id IS NOT NULL)
    ),
    CONSTRAINT gifts_canceled_check CHECK ((status = 'canceled') = (canceled_at IS NOT NULL))
);
CREATE INDEX gifts_giver_idx ON gifts (giver_id, id DESC);
CREATE INDEX gifts_redeemed_by_idx ON gifts (redeemed_by) WHERE redeemed_by IS NOT NULL;
CREATE INDEX gifts_list_idx ON gifts (id DESC);
CREATE TRIGGER gifts_touch BEFORE UPDATE ON gifts FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE promo_links (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    code text NOT NULL CHECK (code ~ '^[A-Za-z0-9_-]{4,32}$'),
    name text NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    squads uuid[] NOT NULL CHECK (cardinality(squads) > 0),
    days integer NOT NULL CHECK (days BETWEEN 1 AND 3650),
    traffic_limit_bytes bigint CHECK (traffic_limit_bytes > 0),
    traffic_reset text NOT NULL DEFAULT 'no_reset'
        CHECK (traffic_reset IN ('no_reset', 'day', 'week', 'month', 'month_rolling', 'on_payment')),
    devices integer NOT NULL CHECK (devices BETWEEN 0 AND 100),
    max_activations integer CHECK (max_activations > 0),
    activations integer NOT NULL DEFAULT 0 CHECK (activations >= 0),
    expires_at timestamptz,
    enabled boolean NOT NULL DEFAULT true,
    created_by bigint REFERENCES admins (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    disabled_at timestamptz,
    CONSTRAINT promo_links_code_key UNIQUE (code),
    CONSTRAINT promo_links_limit_check CHECK (max_activations IS NULL OR activations <= max_activations)
);
CREATE INDEX promo_links_list_idx ON promo_links (id DESC);
CREATE TRIGGER promo_links_touch BEFORE UPDATE ON promo_links FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE promo_link_activations (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    link_id bigint NOT NULL REFERENCES promo_links (id),
    client_id bigint NOT NULL REFERENCES clients (id),
    order_id bigint NOT NULL REFERENCES orders (id),
    activated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT promo_link_activations_client_key UNIQUE (link_id, client_id),
    CONSTRAINT promo_link_activations_order_key UNIQUE (order_id)
);
CREATE INDEX promo_link_activations_client_idx ON promo_link_activations (client_id);
