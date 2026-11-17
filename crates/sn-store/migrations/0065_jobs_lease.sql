ALTER TABLE jobs ADD COLUMN lease_token uuid;
ALTER TABLE jobs DROP CONSTRAINT jobs_dedupe_key;
CREATE UNIQUE INDEX jobs_dedupe_key ON jobs (dedupe_key) WHERE status IN ('queued', 'running');
CREATE INDEX jobs_dedupe_history_idx ON jobs (dedupe_key, id DESC) WHERE dedupe_key IS NOT NULL;
