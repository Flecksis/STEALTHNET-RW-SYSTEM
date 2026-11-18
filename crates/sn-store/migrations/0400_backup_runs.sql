CREATE TABLE backup_runs (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('auto', 'manual', 'before_restore', 'restore')),
    state text NOT NULL CHECK (state IN ('running', 'done', 'failed')),
    slot timestamptz,
    file text,
    size_bytes bigint,
    sha256 text,
    encrypted boolean NOT NULL DEFAULT false,
    with_key boolean NOT NULL DEFAULT false,
    details jsonb NOT NULL DEFAULT '{}'::jsonb,
    error text,
    admin_id bigint,
    started_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz
);
CREATE UNIQUE INDEX backup_runs_slot_key ON backup_runs (slot) WHERE slot IS NOT NULL;
CREATE INDEX backup_runs_file_idx ON backup_runs (file, id DESC) WHERE file IS NOT NULL;
