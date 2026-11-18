CREATE TABLE ext_modules (
    id text PRIMARY KEY CHECK (id ~ '^[a-z][a-z0-9-]{1,39}$'),
    manifest jsonb NOT NULL,
    events text[] NOT NULL DEFAULT '{}',
    enabled boolean NOT NULL DEFAULT true,
    config jsonb NOT NULL DEFAULT '{}',
    api_key_id bigint REFERENCES api_keys (id),
    installed_by bigint NOT NULL REFERENCES admins (id),
    installed_at timestamptz NOT NULL DEFAULT now(),
    updated_by bigint REFERENCES admins (id),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX ext_modules_api_key_idx ON ext_modules (api_key_id) WHERE api_key_id IS NOT NULL;

CREATE TABLE ext_module_deliveries (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    module_id text NOT NULL REFERENCES ext_modules (id) ON DELETE CASCADE,
    event_id bigint,
    kind text NOT NULL,
    payload jsonb NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'done', 'failed', 'skipped')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at timestamptz,
    last_attempt_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT ext_module_deliveries_event_key UNIQUE (module_id, event_id)
);
CREATE INDEX ext_module_deliveries_due_idx ON ext_module_deliveries (module_id, next_attempt_at NULLS FIRST, id) WHERE status = 'pending';
CREATE INDEX ext_module_deliveries_done_idx ON ext_module_deliveries (created_at) WHERE status <> 'pending';

CREATE TABLE ext_module_attempts (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    delivery_id bigint NOT NULL REFERENCES ext_module_deliveries (id) ON DELETE CASCADE,
    module_id text NOT NULL REFERENCES ext_modules (id) ON DELETE CASCADE,
    attempt integer NOT NULL CHECK (attempt > 0),
    at timestamptz NOT NULL DEFAULT now(),
    http_status integer,
    duration_ms integer NOT NULL DEFAULT 0 CHECK (duration_ms >= 0),
    error text,
    response text
);
CREATE INDEX ext_module_attempts_module_idx ON ext_module_attempts (module_id, id DESC);
CREATE INDEX ext_module_attempts_delivery_idx ON ext_module_attempts (delivery_id);
