ALTER TABLE admin_sessions ADD COLUMN method text CHECK (method IN ('password', 'totp'));
