\set ON_ERROR_STOP on
\timing on
SET lock_timeout = '5s';
SET maintenance_work_mem = '64MB';
SET max_parallel_maintenance_workers = 2;

\echo '== 0210: пересоздание CHECK у orders (ACCESS EXCLUSIVE на весь проход)'
BEGIN;
ALTER TABLE orders DROP CONSTRAINT orders_kind_check;
ALTER TABLE orders ADD CONSTRAINT orders_kind_check
    CHECK (kind IN ('new', 'renewal', 'change', 'topup', 'trial', 'module', 'admin_grant', 'module_grant'));
ROLLBACK;

\echo '== то же безопасно: NOT VALID сразу, проверка отдельно (SHARE UPDATE EXCLUSIVE)'
BEGIN;
ALTER TABLE orders DROP CONSTRAINT orders_kind_check;
ALTER TABLE orders ADD CONSTRAINT orders_kind_check
    CHECK (kind IN ('new', 'renewal', 'change', 'topup', 'trial', 'module', 'admin_grant', 'module_grant')) NOT VALID;
ALTER TABLE orders VALIDATE CONSTRAINT orders_kind_check;
ROLLBACK;

\echo '== 0240: CREATE INDEX на payments (SHARE, запись в payments ждёт)'
BEGIN;
CREATE INDEX perf_probe_payments_created ON payments (created_at);
ROLLBACK;

\echo '== 0085: UPDATE по всему webhook_inbox в миграции'
BEGIN;
UPDATE webhook_inbox SET outcome = outcome WHERE outcome IN ('accepted', 'duplicate');
ROLLBACK;

\echo '== 0290: ADD COLUMN с постоянным значением по умолчанию (без перезаписи)'
BEGIN;
ALTER TABLE webhook_inbox ADD COLUMN perf_probe jsonb NOT NULL DEFAULT '[]'::jsonb;
ROLLBACK;

\echo '== 0110: ADD COLUMN ... GENERATED AS IDENTITY (перезапись таблицы) на audit_log'
BEGIN;
ALTER TABLE audit_log ADD COLUMN perf_probe bigint GENERATED ALWAYS AS IDENTITY;
ROLLBACK;

\echo '== 0002: ADD COLUMN и UPDATE всех строк client_identities'
BEGIN;
ALTER TABLE client_identities ADD COLUMN perf_probe text NOT NULL DEFAULT '';
UPDATE client_identities SET perf_probe = lower(value);
ROLLBACK;

\echo '== 0171: ADD PRIMARY KEY на broadcast_recipients (уникальный индекс под ACCESS EXCLUSIVE)'
BEGIN;
ALTER TABLE broadcast_recipients DROP CONSTRAINT broadcast_recipients_pkey;
ALTER TABLE broadcast_recipients ADD PRIMARY KEY (broadcast_id, channel, client_id);
ROLLBACK;

\echo '== 0251: UPDATE tickets и пересоздание CHECK'
BEGIN;
ALTER TABLE tickets DROP CONSTRAINT tickets_ai_reason_check;
UPDATE tickets SET ai_reason = 'no_answer' WHERE ai_reason = 'groq';
ALTER TABLE tickets ADD CONSTRAINT tickets_ai_reason_check
    CHECK (ai_reason IN ('human', 'money', 'unsure', 'no_answer', 'groq', 'client_limit', 'budget', 'operator', 'disabled'));
ROLLBACK;
