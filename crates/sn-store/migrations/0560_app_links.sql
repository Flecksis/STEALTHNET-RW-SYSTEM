CREATE TABLE app_links (
    token_hash bytea PRIMARY KEY,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    subscription_id bigint NOT NULL REFERENCES subscriptions (id) ON DELETE CASCADE,
    spot jsonb NOT NULL,
    holder_hash bytea,
    created_at timestamptz NOT NULL DEFAULT now(),
    opened_at timestamptz,
    expires_at timestamptz NOT NULL,
    CHECK ((holder_hash IS NULL) = (opened_at IS NULL))
);
CREATE INDEX app_links_expires_idx ON app_links (expires_at);
