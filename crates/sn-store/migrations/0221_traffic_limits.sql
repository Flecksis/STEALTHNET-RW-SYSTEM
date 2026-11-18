CREATE TABLE traffic_limit_rules (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    enabled boolean NOT NULL DEFAULT true,
    audience text NOT NULL DEFAULT 'all' CHECK (audience IN ('all', 'trial', 'paid')),
    traffic text NOT NULL DEFAULT 'any' CHECK (traffic IN ('any', 'unlimited', 'limited')),
    devices_min integer CHECK (devices_min > 0),
    period text NOT NULL CHECK (period IN ('day', 'week')),
    limit_bytes bigint NOT NULL CHECK (limit_bytes > 0),
    per_device boolean NOT NULL DEFAULT false,
    warn boolean NOT NULL DEFAULT true,
    alert boolean NOT NULL DEFAULT true,
    restriction text NOT NULL DEFAULT 'none' CHECK (restriction IN ('none', 'disable', 'squad')),
    squad_uuid uuid,
    note text,
    created_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT traffic_limit_rules_squad_check CHECK (restriction <> 'squad' OR squad_uuid IS NOT NULL)
);
CREATE TRIGGER traffic_limit_rules_touch BEFORE UPDATE ON traffic_limit_rules FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE traffic_counters (
    subscription_id bigint PRIMARY KEY REFERENCES subscriptions (id) ON DELETE CASCADE,
    day date NOT NULL,
    day_base bigint NOT NULL,
    week date NOT NULL,
    week_base bigint NOT NULL,
    last_bytes bigint NOT NULL,
    seen_at timestamptz NOT NULL
);

CREATE TABLE traffic_exceeds (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    rule_id bigint NOT NULL REFERENCES traffic_limit_rules (id),
    subscription_id bigint NOT NULL REFERENCES subscriptions (id) ON DELETE CASCADE,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    period text NOT NULL CHECK (period IN ('day', 'week')),
    window_start date NOT NULL,
    window_end timestamptz NOT NULL,
    limit_bytes bigint NOT NULL,
    used_bytes bigint NOT NULL,
    warn boolean NOT NULL,
    alert boolean NOT NULL,
    restriction text NOT NULL CHECK (restriction IN ('none', 'disable', 'squad')),
    squad_uuid uuid,
    previous_squads jsonb,
    restrict_tries integer NOT NULL DEFAULT 0,
    exceeded_at timestamptz NOT NULL,
    warned_at timestamptz,
    restricted_at timestamptz,
    restrict_note text,
    lifted_at timestamptz,
    lift_reason text CHECK (lift_reason IN ('window', 'admin', 'ended', 'blocked', 'changed', 'off')),
    lifted_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    lift_due_at timestamptz,
    CONSTRAINT traffic_exceeds_window_key UNIQUE (rule_id, subscription_id, window_start)
);
CREATE INDEX traffic_exceeds_client_idx ON traffic_exceeds (client_id, id DESC);
CREATE INDEX traffic_exceeds_window_idx ON traffic_exceeds (window_end DESC);
CREATE INDEX traffic_exceeds_restricted_idx ON traffic_exceeds (lift_due_at) WHERE restricted_at IS NOT NULL AND lifted_at IS NULL;
