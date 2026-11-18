CREATE TABLE analytics_spans (
    order_id bigint PRIMARY KEY,
    client_id bigint NOT NULL,
    paid_at timestamptz NOT NULL,
    starts_at timestamptz NOT NULL,
    ends_at timestamptz NOT NULL,
    monthly_minor bigint NOT NULL,
    continues boolean NOT NULL,
    CHECK (ends_at >= starts_at)
);
CREATE INDEX analytics_spans_client_idx ON analytics_spans (client_id);
CREATE INDEX analytics_spans_paid_idx ON analytics_spans (paid_at);

CREATE TABLE analytics_islands (
    client_id bigint NOT NULL,
    seq integer NOT NULL CHECK (seq > 0),
    starts_at timestamptz NOT NULL,
    ends_at timestamptz NOT NULL,
    PRIMARY KEY (client_id, seq),
    CHECK (ends_at >= starts_at)
);
CREATE INDEX analytics_islands_starts_idx ON analytics_islands (starts_at);
CREATE INDEX analytics_islands_ends_idx ON analytics_islands (ends_at);

CREATE TABLE analytics_state (
    id boolean PRIMARY KEY DEFAULT true CHECK (id),
    currency text NOT NULL,
    version integer NOT NULL,
    watermark timestamptz NOT NULL,
    refreshed_at timestamptz NOT NULL,
    rebuilt_at timestamptz NOT NULL
);

CREATE INDEX payments_created_at_idx ON payments (created_at);
CREATE INDEX orders_updated_at_idx ON orders (updated_at);
CREATE INDEX orders_trial_paid_idx ON orders (paid_at) WHERE kind = 'trial';
