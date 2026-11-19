CREATE INDEX IF NOT EXISTS subscriptions_stale_mirror_idx ON subscriptions (synced_at NULLS FIRST)
    WHERE ended_at IS NULL AND remna_user_id IS NOT NULL;
