CREATE TABLE auto_broadcasts (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    trigger text NOT NULL CHECK (trigger IN ('expiring', 'expired', 'trial_ended', 'no_purchase', 'not_connected')),
    days integer NOT NULL CHECK (days BETWEEN 0 AND 90),
    with_autorenew boolean NOT NULL DEFAULT false,
    channel text NOT NULL DEFAULT 'bot' CHECK (channel IN ('bot', 'email', 'both')),
    subject text,
    text text NOT NULL CHECK (btrim(text) <> ''),
    image_id bigint REFERENCES broadcast_images (id),
    buttons jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(buttons) = 'array'),
    enabled boolean NOT NULL DEFAULT false,
    version integer NOT NULL DEFAULT 1 CHECK (version > 0),
    notice text,
    checked_at timestamptz,
    created_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz
);
CREATE INDEX auto_broadcasts_live_idx ON auto_broadcasts (id) WHERE deleted_at IS NULL;
CREATE TRIGGER auto_broadcasts_touch BEFORE UPDATE ON auto_broadcasts FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

ALTER TABLE broadcasts ADD COLUMN rule_id bigint REFERENCES auto_broadcasts (id);
ALTER TABLE broadcasts ADD COLUMN rule_day date;
ALTER TABLE broadcasts ADD COLUMN rule_version integer;
ALTER TABLE broadcasts ADD CONSTRAINT broadcasts_rule_check CHECK (
    (rule_id IS NULL AND rule_day IS NULL AND rule_version IS NULL)
    OR (rule_id IS NOT NULL AND rule_day IS NOT NULL AND rule_version IS NOT NULL)
);
CREATE UNIQUE INDEX broadcasts_rule_day_key ON broadcasts (rule_id, rule_day, rule_version) WHERE rule_id IS NOT NULL;
CREATE INDEX broadcasts_once_idx ON broadcasts (id DESC) WHERE rule_id IS NULL;

CREATE TABLE auto_broadcast_cases (
    rule_id bigint NOT NULL REFERENCES auto_broadcasts (id) ON DELETE CASCADE,
    case_key text NOT NULL CHECK (case_key <> ''),
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    due_at timestamptz NOT NULL,
    broadcast_id bigint REFERENCES broadcasts (id) ON DELETE SET NULL,
    queued boolean NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (rule_id, case_key)
);
CREATE INDEX auto_broadcast_cases_client_idx ON auto_broadcast_cases (client_id);

CREATE INDEX subscriptions_expire_any_idx ON subscriptions (expire_at);
