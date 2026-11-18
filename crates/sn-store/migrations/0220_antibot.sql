CREATE FUNCTION antibot_email_key(raw text) RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    WITH parts AS (
        SELECT split_part(lower(btrim(raw)), '@', 1) AS local,
               split_part(lower(btrim(raw)), '@', 2) AS domain
    ), plain AS (
        SELECT CASE WHEN split_part(local, '+', 1) = '' THEN local ELSE split_part(local, '+', 1) END AS local,
               CASE
                   WHEN domain = 'googlemail.com' THEN 'gmail.com'
                   WHEN domain IN ('ya.ru', 'yandex.com', 'yandex.by', 'yandex.kz', 'yandex.ua') THEN 'yandex.ru'
                   ELSE domain
               END AS domain
        FROM parts
    )
    SELECT CASE
               WHEN domain = 'gmail.com' THEN replace(local, '.', '')
               WHEN domain = 'yandex.ru' THEN replace(local, '.', '-')
               ELSE local
           END || '@' || domain
    FROM plain
$$;

CREATE INDEX client_identities_email_key_idx ON client_identities (antibot_email_key(value)) WHERE kind = 'email';
CREATE INDEX client_identities_telegram_created_idx ON client_identities (created_at) WHERE kind = 'telegram';
CREATE INDEX client_sessions_client_created_idx ON client_sessions (client_id, created_at);

CREATE TABLE antibot_marks (
    client_id bigint PRIMARY KEY REFERENCES clients (id) ON DELETE CASCADE,
    reasons jsonb NOT NULL DEFAULT '[]'::jsonb,
    details jsonb NOT NULL DEFAULT '{}'::jsonb,
    marked_at timestamptz NOT NULL DEFAULT now(),
    confirmed_at timestamptz,
    cleared_at timestamptz,
    cleared_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    challenge_hash bytea,
    challenge_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX antibot_marks_open_idx ON antibot_marks (marked_at DESC) WHERE cleared_at IS NULL;
CREATE TRIGGER antibot_marks_touch BEFORE UPDATE ON antibot_marks FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE antibot_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    action text NOT NULL CHECK (action IN ('marked', 'asked', 'confirmed', 'trial_denied', 'promo_denied', 'no_bonus', 'cleared')),
    reasons jsonb NOT NULL DEFAULT '[]'::jsonb,
    details jsonb NOT NULL DEFAULT '{}'::jsonb,
    dedupe text,
    admin_id bigint REFERENCES admins (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX antibot_events_dedupe_key ON antibot_events (dedupe) WHERE dedupe IS NOT NULL;
CREATE INDEX antibot_events_client_idx ON antibot_events (client_id, id DESC);
CREATE INDEX antibot_events_action_idx ON antibot_events (action, id DESC);
