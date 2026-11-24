CREATE TABLE email_signups (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email text NOT NULL,
    token_hash bytea NOT NULL,
    code_hash bytea NOT NULL,
    password_hash text,
    taken boolean NOT NULL DEFAULT false,
    locale text NOT NULL CHECK (locale IN ('ru', 'en')),
    start_param text,
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    ip text,
    user_agent text,
    created_at timestamptz NOT NULL DEFAULT now(),
    code_expires_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    CHECK (taken OR password_hash IS NOT NULL)
);
CREATE UNIQUE INDEX email_signups_token_key ON email_signups (token_hash);
CREATE INDEX email_signups_email_idx ON email_signups (email);
CREATE INDEX email_signups_expires_idx ON email_signups (expires_at);
