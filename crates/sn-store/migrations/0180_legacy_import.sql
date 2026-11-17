CREATE TABLE import_runs (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id uuid NOT NULL DEFAULT gen_random_uuid(),
    source text NOT NULL CHECK (source IN ('stealthnet', 'bedolaga')),
    state text NOT NULL CHECK (state IN ('uploaded', 'checking', 'needs_password', 'ready', 'failed', 'running', 'done', 'interrupted', 'discarded')),
    file_name text NOT NULL,
    file_size bigint NOT NULL CHECK (file_size >= 0),
    file_path text,
    preview jsonb,
    progress jsonb,
    outcome jsonb,
    error text,
    created_by bigint REFERENCES admins (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz,
    CONSTRAINT import_runs_public_id_key UNIQUE (public_id)
);
CREATE UNIQUE INDEX import_runs_running_key ON import_runs ((true)) WHERE state = 'running';
CREATE INDEX import_runs_recent_idx ON import_runs (id DESC);
CREATE TRIGGER import_runs_touch BEFORE UPDATE ON import_runs FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE import_issues (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    run_id bigint NOT NULL REFERENCES import_runs (id) ON DELETE CASCADE,
    stage text NOT NULL CHECK (stage IN ('check', 'apply')),
    code text NOT NULL,
    entity text NOT NULL,
    old_id text,
    who text,
    details text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX import_issues_run_idx ON import_issues (run_id, stage, id);

ALTER TABLE legacy_ids
    ADD COLUMN data jsonb NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN run_id bigint REFERENCES import_runs (id) ON DELETE SET NULL,
    ADD COLUMN updated_at timestamptz NOT NULL DEFAULT now();
CREATE INDEX legacy_ids_new_idx ON legacy_ids (entity, new_id);
