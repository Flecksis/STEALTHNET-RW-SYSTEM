ALTER TABLE analytics_clients ADD COLUMN connected_at timestamptz;

CREATE INDEX subscriptions_first_connected_idx ON subscriptions (first_connected_at) WHERE first_connected_at IS NOT NULL;
