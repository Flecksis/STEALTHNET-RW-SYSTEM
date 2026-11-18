ALTER TABLE subscriptions ADD COLUMN device_limit integer CHECK (device_limit > 0);
