ALTER TABLE orders ADD COLUMN plan_applied_at timestamptz;

CREATE INDEX orders_subscription_fulfill_idx ON orders (subscription_id, id) WHERE subscription_id IS NOT NULL;
