ALTER TABLE orders ADD COLUMN channel text CHECK (channel IN ('bot', 'miniapp', 'cabinet', 'admin', 'autorenew'));

ALTER TABLE analytics_spans
    ADD COLUMN kind text,
    ADD COLUMN tariff_id bigint,
    ADD COLUMN period_days integer,
    ADD COLUMN gap_seconds bigint;

CREATE INDEX analytics_spans_ends_idx ON analytics_spans (ends_at);

CREATE TABLE analytics_payments (
    payment_id bigint PRIMARY KEY,
    client_id bigint NOT NULL,
    order_id bigint NOT NULL,
    paid_at timestamptz NOT NULL,
    kind text NOT NULL,
    item text NOT NULL DEFAULT '',
    tariff_id bigint,
    category_id bigint,
    period_days integer,
    provider text NOT NULL,
    method text NOT NULL DEFAULT '',
    channel text NOT NULL DEFAULT '',
    stars boolean NOT NULL DEFAULT false,
    amount_minor bigint NOT NULL,
    refunded_minor bigint NOT NULL DEFAULT 0,
    promo_minor bigint NOT NULL DEFAULT 0,
    promo_code text,
    discount_minor bigint NOT NULL DEFAULT 0,
    addon_minor bigint NOT NULL DEFAULT 0
);

CREATE INDEX analytics_payments_paid_idx ON analytics_payments (paid_at);
CREATE INDEX analytics_payments_client_idx ON analytics_payments (client_id, paid_at);

CREATE INDEX payments_updated_at_idx ON payments (updated_at);

CREATE TABLE analytics_clients (
    client_id bigint PRIMARY KEY,
    created_at timestamptz NOT NULL,
    reg_channel text NOT NULL,
    login_kind text NOT NULL DEFAULT '',
    source_type text NOT NULL DEFAULT 'none',
    source_key text NOT NULL DEFAULT '',
    utm_medium text NOT NULL DEFAULT '',
    utm_campaign text NOT NULL DEFAULT '',
    first_paid_at timestamptz,
    deleted boolean NOT NULL DEFAULT false
);

CREATE INDEX analytics_clients_created_idx ON analytics_clients (created_at);
CREATE INDEX analytics_clients_first_paid_idx ON analytics_clients (first_paid_at) WHERE first_paid_at IS NOT NULL;

CREATE TABLE analytics_snapshots (
    day date NOT NULL,
    key text NOT NULL,
    value bigint NOT NULL,
    PRIMARY KEY (day, key)
);

CREATE TABLE analytics_node_samples (
    at timestamptz NOT NULL,
    node_uuid uuid NOT NULL,
    name text NOT NULL,
    online integer NOT NULL CHECK (online >= 0),
    PRIMARY KEY (at, node_uuid)
);

CREATE TABLE utm_links (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    title text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 100),
    source text NOT NULL CHECK (source ~ '^[A-Za-z0-9-]+(_[A-Za-z0-9-]+)*$' AND length(source) <= 64),
    medium text CHECK (medium ~ '^[A-Za-z0-9-]+(_[A-Za-z0-9-]+)*$' AND length(medium) <= 64),
    campaign text CHECK (campaign ~ '^[A-Za-z0-9-]+(_[A-Za-z0-9-]+)*$' AND length(campaign) <= 64),
    content text CHECK (content ~ '^[A-Za-z0-9-]+(_[A-Za-z0-9-]+)*$' AND length(content) <= 64),
    term text CHECK (term ~ '^[A-Za-z0-9-]+(_[A-Za-z0-9-]+)*$' AND length(term) <= 64),
    created_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz
);

CREATE UNIQUE INDEX utm_links_tags_key ON utm_links
    (source, COALESCE(medium, ''), COALESCE(campaign, ''), COALESCE(content, ''), COALESCE(term, ''))
    WHERE archived_at IS NULL;
CREATE TRIGGER utm_links_touch BEFORE UPDATE ON utm_links FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE utm_clicks (
    at timestamptz NOT NULL,
    channel text NOT NULL CHECK (channel IN ('bot', 'miniapp', 'site')),
    source text NOT NULL DEFAULT '',
    medium text NOT NULL DEFAULT '',
    campaign text NOT NULL DEFAULT '',
    content text NOT NULL DEFAULT '',
    term text NOT NULL DEFAULT '',
    clicks integer NOT NULL DEFAULT 0 CHECK (clicks >= 0),
    PRIMARY KEY (at, channel, source, medium, campaign, content, term)
);

ALTER TABLE oauth_requests ADD COLUMN utm text CHECK (length(utm) <= 512);
