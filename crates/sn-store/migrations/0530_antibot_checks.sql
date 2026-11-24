CREATE TABLE antibot_checks (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    target text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('button', 'work', 'captcha')),
    token_hash bytea NOT NULL,
    bits smallint NOT NULL DEFAULT 0 CHECK (bits BETWEEN 0 AND 32),
    failures integer NOT NULL DEFAULT 0 CHECK (failures >= 0),
    details jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    passed_at timestamptz
);
CREATE UNIQUE INDEX antibot_checks_token_key ON antibot_checks (token_hash);
CREATE INDEX antibot_checks_client_idx ON antibot_checks (client_id, target, created_at DESC);
CREATE INDEX antibot_checks_expires_idx ON antibot_checks (expires_at);

CREATE TABLE antibot_captcha_spent (
    token_hash bytea PRIMARY KEY,
    spent_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX antibot_captcha_spent_at_idx ON antibot_captcha_spent (spent_at);

ALTER TABLE antibot_marks DROP COLUMN challenge_hash, DROP COLUMN challenge_at;
