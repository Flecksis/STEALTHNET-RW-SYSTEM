CREATE TABLE analytics_attempts (
    payment_id bigint PRIMARY KEY,
    client_id bigint NOT NULL,
    created_at timestamptz NOT NULL,
    provider text NOT NULL,
    method text NOT NULL DEFAULT '',
    channel text NOT NULL DEFAULT '',
    status text NOT NULL,
    expires_at timestamptz,
    wait_seconds bigint,
    amount_minor bigint NOT NULL DEFAULT 0,
    refunded_minor bigint NOT NULL DEFAULT 0,
    reason text
);

CREATE INDEX analytics_attempts_created_idx ON analytics_attempts (created_at);

CREATE INDEX analytics_spans_client_paid_idx ON analytics_spans (client_id, paid_at);

DROP INDEX analytics_spans_client_idx;

ALTER TABLE analytics_clients
    ADD COLUMN trial_at timestamptz,
    ADD COLUMN first_island_at timestamptz,
    ADD COLUMN kept30 boolean NOT NULL DEFAULT false;
