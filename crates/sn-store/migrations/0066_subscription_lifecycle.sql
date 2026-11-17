CREATE INDEX subscriptions_autorenew_due_idx ON subscriptions (expire_at, id) WHERE ended_at IS NULL AND autorenew_enabled AND expire_at IS NOT NULL;
CREATE INDEX orders_open_subscription_idx ON orders (subscription_id) WHERE status IN ('pending', 'paid', 'review');
