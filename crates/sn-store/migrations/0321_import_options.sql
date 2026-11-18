ALTER TABLE import_runs ADD COLUMN options jsonb NOT NULL DEFAULT '{}'::jsonb;
