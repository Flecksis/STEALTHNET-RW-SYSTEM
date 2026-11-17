ALTER TABLE client_login_requests DROP CONSTRAINT client_login_requests_method_check;
ALTER TABLE client_login_requests ADD CONSTRAINT client_login_requests_method_check
    CHECK (method IN ('telegram', 'email', 'totp', 'reset'));
ALTER TABLE client_login_requests ADD CONSTRAINT client_login_requests_reset_email_check
    CHECK (method <> 'reset' OR email IS NOT NULL);
