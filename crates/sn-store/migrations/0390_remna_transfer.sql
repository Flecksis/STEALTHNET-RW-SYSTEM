CREATE TABLE remna_snapshots (
    subscription_id bigint PRIMARY KEY REFERENCES subscriptions (id) ON DELETE CASCADE,
    remna_user_id bigint,
    username text,
    short_uuid text,
    squads uuid[] NOT NULL DEFAULT '{}',
    squad_names text[] NOT NULL DEFAULT '{}',
    external_squad_uuid uuid,
    tag text,
    telegram_id bigint,
    email text,
    description text,
    remna_created_at timestamptz,
    last_traffic_reset_at timestamptz,
    subscription_url text,
    keys_nonce bytea,
    keys_sealed bytea,
    seen_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE remna_transfers (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id uuid NOT NULL DEFAULT gen_random_uuid(),
    direction text NOT NULL CHECK (direction IN ('push', 'pull')),
    state text NOT NULL CHECK (state IN ('checking', 'ready', 'running', 'paused', 'done', 'failed', 'interrupted', 'discarded')),
    mode text CHECK (mode IN ('create', 'fix')),
    options jsonb NOT NULL DEFAULT '{}'::jsonb,
    plan jsonb,
    progress jsonb,
    outcome jsonb,
    error text,
    pause_requested boolean NOT NULL DEFAULT false,
    created_by bigint REFERENCES admins (id),
    started_by bigint REFERENCES admins (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz,
    CONSTRAINT remna_transfers_public_id_key UNIQUE (public_id)
);
CREATE UNIQUE INDEX remna_transfers_busy_key ON remna_transfers ((true)) WHERE state IN ('checking', 'running');
CREATE INDEX remna_transfers_recent_idx ON remna_transfers (id DESC);
CREATE INDEX remna_transfers_finished_idx ON remna_transfers (finished_at) WHERE finished_at IS NOT NULL;
CREATE TRIGGER remna_transfers_touch BEFORE UPDATE ON remna_transfers FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE remna_transfer_notes (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    transfer_id bigint NOT NULL REFERENCES remna_transfers (id) ON DELETE CASCADE,
    stage text NOT NULL CHECK (stage IN ('check', 'apply')),
    code text NOT NULL,
    client_id bigint,
    subscription_id bigint,
    remna_user_id bigint,
    who text,
    details text NOT NULL,
    data jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX remna_transfer_notes_run_idx ON remna_transfer_notes (transfer_id, stage, id);
CREATE INDEX remna_transfer_notes_code_idx ON remna_transfer_notes (transfer_id, code);
