ALTER TABLE audit_log ADD COLUMN api_key_id bigint, ADD COLUMN ext_module text,
    ADD COLUMN via_recorded boolean NOT NULL DEFAULT false;
ALTER TABLE audit_log ALTER COLUMN via_recorded SET DEFAULT true;

CREATE INDEX audit_log_time_idx ON audit_log (created_at DESC, id DESC);
CREATE INDEX audit_log_actor_time_idx ON audit_log (actor_kind, actor_id, created_at DESC, id DESC);
CREATE INDEX audit_log_action_time_idx ON audit_log (action, created_at DESC, id DESC);
CREATE INDEX audit_log_key_time_idx ON audit_log (api_key_id, created_at DESC, id DESC) WHERE api_key_id IS NOT NULL;
CREATE INDEX audit_log_module_time_idx ON audit_log (ext_module, created_at DESC, id DESC) WHERE ext_module IS NOT NULL;
CREATE INDEX audit_log_ip_time_idx ON audit_log (ip, created_at DESC, id DESC) WHERE ip IS NOT NULL;

DO $$
DECLARE
    trgm text;
BEGIN
    BEGIN
        CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;
    EXCEPTION WHEN OTHERS THEN
        RAISE NOTICE 'Расширение pg_trgm недоступно (%): поиск в журнале действий будет работать без индекса', SQLERRM;
    END;
    SELECT n.nspname INTO trgm
    FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
    WHERE e.extname = 'pg_trgm';
    IF trgm IS NOT NULL THEN
        EXECUTE format(
            'CREATE INDEX audit_log_search_idx ON audit_log USING gin ((details::text) %I.gin_trgm_ops)',
            trgm);
    END IF;
END $$;

ANALYZE audit_log;
