CREATE TABLE client_login_requests (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    method text NOT NULL CHECK (method IN ('telegram', 'email')),
    code_hash bytea NOT NULL,
    poll_hash bytea,
    email text,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'confirmed', 'rejected', 'consumed')),
    client_id bigint REFERENCES clients (id) ON DELETE CASCADE,
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    ip text,
    user_agent text,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    confirmed_at timestamptz,
    consumed_at timestamptz,
    CHECK (method <> 'email' OR email IS NOT NULL),
    CHECK (method <> 'telegram' OR poll_hash IS NOT NULL)
);
CREATE INDEX client_login_requests_code_idx ON client_login_requests (code_hash);
CREATE INDEX client_login_requests_poll_idx ON client_login_requests (poll_hash) WHERE poll_hash IS NOT NULL;
CREATE INDEX client_login_requests_email_idx ON client_login_requests (email, id DESC) WHERE email IS NOT NULL;
CREATE INDEX client_login_requests_expires_idx ON client_login_requests (expires_at);
