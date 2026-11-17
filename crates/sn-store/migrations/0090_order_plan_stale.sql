ALTER TABLE orders ADD COLUMN plan_stale_at timestamptz;
UPDATE orders SET plan_stale_at = now() WHERE status = 'review' AND plan IS NOT NULL AND plan_applied_at IS NULL;
