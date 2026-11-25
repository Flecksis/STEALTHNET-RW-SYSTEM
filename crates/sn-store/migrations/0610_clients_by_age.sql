CREATE INDEX clients_age_idx ON clients (created_at DESC, id DESC) WHERE deleted_at IS NULL;
DROP INDEX clients_created_at_idx;
