ALTER TABLE client_login_requests DROP CONSTRAINT client_login_requests_method_check;
ALTER TABLE client_login_requests ADD CONSTRAINT client_login_requests_method_check
    CHECK (method IN ('telegram', 'email', 'totp', 'reset', 'email_change'));
ALTER TABLE client_login_requests ADD CONSTRAINT client_login_requests_email_change_check
    CHECK (method <> 'email_change' OR (email IS NOT NULL AND client_id IS NOT NULL));
CREATE INDEX client_login_requests_email_change_idx ON client_login_requests (client_id, id DESC)
    WHERE method = 'email_change' AND status = 'pending';
