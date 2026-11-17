CREATE TABLE api_keys (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    token_prefix text NOT NULL,
    token_hash bytea NOT NULL,
    permissions text[] NOT NULL DEFAULT '{}',
    created_by bigint NOT NULL REFERENCES admins (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz,
    last_used_at timestamptz,
    revoked_at timestamptz,
    CONSTRAINT api_keys_token_key UNIQUE (token_hash)
);
CREATE INDEX api_keys_created_by_idx ON api_keys (created_by);
