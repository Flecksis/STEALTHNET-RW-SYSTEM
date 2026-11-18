DO $$
DECLARE
    trgm text;
BEGIN
    BEGIN
        CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;
    EXCEPTION WHEN OTHERS THEN
        RAISE NOTICE 'Расширение pg_trgm недоступно (%): поиск в журнале уведомлений будет работать без индекса', SQLERRM;
    END;
    SELECT n.nspname INTO trgm
    FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
    WHERE e.extname = 'pg_trgm';
    IF trgm IS NOT NULL THEN
        EXECUTE format(
            'CREATE INDEX webhook_inbox_search_idx ON webhook_inbox USING gin ('
            '(parsed ->> %1$L) %2$I.gin_trgm_ops, '
            '(parsed ->> %3$L) %2$I.gin_trgm_ops, '
            '((parsed -> %4$L) ->> %5$L) %2$I.gin_trgm_ops, '
            'encode(body, %6$L) %2$I.gin_trgm_ops)',
            'provider_txid', trgm, 'event_key', 'payment', 'value', 'escape');
    END IF;
END $$;

ANALYZE webhook_inbox;
