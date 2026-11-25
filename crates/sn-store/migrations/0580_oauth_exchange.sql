ALTER TABLE oauth_requests ADD COLUMN exchange_at timestamptz;
ALTER TABLE oauth_requests DROP CONSTRAINT oauth_requests_status_check;
ALTER TABLE oauth_requests ADD CONSTRAINT oauth_requests_status_check
    CHECK (status IN ('pending', 'exchanging', 'human', 'done', 'failed', 'consumed'));
ALTER TABLE oauth_requests ADD CONSTRAINT oauth_requests_exchange_check
    CHECK (status <> 'exchanging' OR exchange_at IS NOT NULL);
