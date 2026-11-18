CREATE TABLE oauth_requests (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    provider text NOT NULL CHECK (provider IN ('google', 'apple')),
    purpose text NOT NULL CHECK (purpose IN ('login', 'link')),
    state_hash bytea NOT NULL,
    browser_hash bytea NOT NULL,
    client_id bigint REFERENCES clients (id) ON DELETE CASCADE,
    session_id bigint REFERENCES client_sessions (id) ON DELETE CASCADE,
    referral text,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'done', 'failed', 'consumed')),
    result_client_id bigint REFERENCES clients (id) ON DELETE CASCADE,
    created_client boolean NOT NULL DEFAULT false,
    error_code text,
    error_text text,
    ip text,
    user_agent text,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    finished_at timestamptz,
    CONSTRAINT oauth_requests_state_key UNIQUE (state_hash),
    CHECK (purpose <> 'link' OR (client_id IS NOT NULL AND session_id IS NOT NULL)),
    CHECK (status <> 'failed' OR error_code IS NOT NULL)
);
CREATE INDEX oauth_requests_browser_idx ON oauth_requests (browser_hash, id DESC);
CREATE INDEX oauth_requests_expires_idx ON oauth_requests (expires_at);

CREATE UNIQUE INDEX client_identities_one_oauth_idx ON client_identities (client_id, kind)
    WHERE kind IN ('google', 'apple');
