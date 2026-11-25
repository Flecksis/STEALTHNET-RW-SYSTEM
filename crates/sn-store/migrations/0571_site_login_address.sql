CREATE INDEX client_login_requests_client_idx ON client_login_requests (client_id)
    WHERE method = 'telegram' AND client_id IS NOT NULL;
CREATE INDEX client_login_requests_telegram_idx ON client_login_requests (created_at)
    WHERE method = 'telegram' AND client_id IS NOT NULL;
