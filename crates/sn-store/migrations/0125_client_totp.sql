ALTER TABLE client_credentials ADD COLUMN totp_enabled_at timestamptz;

ALTER TABLE client_login_requests DROP CONSTRAINT client_login_requests_method_check;
ALTER TABLE client_login_requests ADD CONSTRAINT client_login_requests_method_check
    CHECK (method IN ('telegram', 'email', 'totp'));
CREATE INDEX client_login_requests_totp_client_idx ON client_login_requests (client_id)
    WHERE method = 'totp' AND status = 'pending';
