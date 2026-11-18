ALTER TABLE import_runs DROP CONSTRAINT IF EXISTS import_runs_source_check;
ALTER TABLE import_runs ADD CONSTRAINT import_runs_source_check CHECK (source ~ '^[a-z][a-z0-9_]{1,39}$');
