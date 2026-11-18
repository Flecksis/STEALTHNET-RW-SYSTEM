\set ON_ERROR_STOP on
\if :{?scale}
\else
\set scale 1
\endif
SET client_min_messages = warning;
SET max_parallel_workers_per_gather = 0;
SET synchronous_commit = off;
SET work_mem = '64MB';
SET maintenance_work_mem = '256MB';

SELECT CASE WHEN EXISTS (SELECT 1 FROM clients) THEN 1 ELSE 0 END AS not_empty \gset
\if :not_empty
\echo 'база уже заполнена: генератор работает только на пустой базе после миграций'
\quit
\endif

SELECT date_trunc('minute', now()) AS t0,
       greatest(1, round(300000 * :scale))::int AS nc,
       greatest(1, round(100000 * :scale))::int AS ntickets,
       greatest(1, round(2000 * :scale))::int AS npromo
\gset

DROP SCHEMA IF EXISTS perf_gen CASCADE;
CREATE SCHEMA perf_gen;

CREATE FUNCTION perf_gen.r(seed bigint, salt integer) RETURNS double precision
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS $$ SELECT (hashint8(seed * 7919 + salt) & 2147483647)::double precision / 2147483648.0 $$;

CREATE FUNCTION perf_gen.pick(items text[], x double precision) RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS $$ SELECT items[1 + least(cardinality(items) - 1, floor(x * cardinality(items))::int)] $$;

CREATE FUNCTION perf_gen.price(tariff integer, pidx integer) RETURNS bigint
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS $$ SELECT (round((ARRAY[19900, 29900, 49900, 99000, 14900, 24900])[tariff] * (ARRAY[1, 2.7, 5, 9])[pidx] / 100.0) * 100)::bigint $$;

CREATE FUNCTION perf_gen.stars(tariff integer, pidx integer) RETURNS bigint
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS $$ SELECT greatest(1, round(perf_gen.price(tariff, pidx) / 130.0))::bigint $$;

CREATE FUNCTION perf_gen.money(minor bigint) RETURNS jsonb
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS $$ SELECT jsonb_build_object('minor', minor, 'currency', 'RUB') $$;

CREATE FUNCTION perf_gen.uuid(seed bigint, salt integer) RETURNS uuid
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS $$ SELECT (substr(m, 1, 8) || '-' || substr(m, 9, 4) || '-4' || substr(m, 14, 3) || '-8' || substr(m, 18, 3) || '-' || substr(m, 21, 12))::uuid
      FROM (SELECT md5(seed::text || ':' || salt::text) AS m) x $$;

CREATE PROCEDURE perf_gen.load(target text, cols text, source text, step bigint DEFAULT 50000, overriding boolean DEFAULT true)
LANGUAGE plpgsql AS $$
DECLARE
    lo bigint;
    hi bigint;
    a bigint;
BEGIN
    EXECUTE format('SELECT min(id), max(id) FROM %s', source) INTO lo, hi;
    IF lo IS NULL THEN
        RETURN;
    END IF;
    EXECUTE format('CREATE INDEX ON %s (id)', source);
    COMMIT;
    a := lo;
    WHILE a <= hi LOOP
        EXECUTE format('INSERT INTO %s (%s) %s SELECT %s FROM %s WHERE id >= %s AND id < %s ORDER BY id',
            target, cols, CASE WHEN overriding THEN 'OVERRIDING SYSTEM VALUE' ELSE '' END, cols, source, a, a + step);
        COMMIT;
        PERFORM pg_sleep(0.02);
        a := a + step;
    END LOOP;
    IF overriding THEN
        EXECUTE format('SELECT setval(pg_get_serial_sequence(%L, ''id''), (SELECT max(id) FROM %s))', target, target);
        COMMIT;
    END IF;
END $$;

INSERT INTO settings (key, value) VALUES
    ('core.currency', '"RUB"'),
    ('core.brand_name', '"Perf VPN"'),
    ('core.public_url', '"http://127.0.0.1:18124"');

INSERT INTO module_states (module, enabled, config)
SELECT m, true, '{}'::jsonb
FROM unnest(ARRAY['promo', 'gifts', 'options', 'broadcast', 'referral', 'promolinks', 'utm', 'contests', 'roulette', 'antibot', 'limits', 'analytics']) m;

INSERT INTO tariff_categories (id, name, sort, visible) OVERRIDING SYSTEM VALUE VALUES
    (1, 'Основные', 1, true),
    (2, 'Архив', 2, false);

INSERT INTO tariffs (id, category_id, name, description, on_sale, sort, devices_included, extra_devices_enabled, extra_devices_max,
                     extra_device_rub_minor, extra_device_stars, traffic_limit_bytes, traffic_reset, created_at)
OVERRIDING SYSTEM VALUE VALUES
    (1, 1, 'Старт', 'Одно устройство', true, 1, 1, true, 2, 9900, 80, NULL, 'no_reset', :'t0'::timestamptz - interval '800 days'),
    (2, 1, 'Стандарт', 'Три устройства', true, 2, 3, true, 3, 9900, 80, NULL, 'no_reset', :'t0'::timestamptz - interval '800 days'),
    (3, 1, 'Семья', 'Пять устройств', true, 3, 5, true, 5, 9900, 80, NULL, 'no_reset', :'t0'::timestamptz - interval '800 days'),
    (4, 1, 'Про', 'Десять устройств', true, 4, 10, false, 0, NULL, NULL, 536870912000, 'month', :'t0'::timestamptz - interval '800 days'),
    (5, 2, 'Старый базовый', NULL, false, 5, 1, false, 0, NULL, NULL, NULL, 'no_reset', :'t0'::timestamptz - interval '900 days'),
    (6, 2, 'Старый семейный', NULL, false, 6, 4, false, 0, NULL, NULL, NULL, 'no_reset', :'t0'::timestamptz - interval '900 days');

INSERT INTO tariff_periods (id, tariff_id, days, price_rub_minor, stars, sort, active)
OVERRIDING SYSTEM VALUE
SELECT (t - 1) * 4 + p, t, (ARRAY[30, 90, 180, 365])[p], perf_gen.price(t, p), perf_gen.stars(t, p), p, true
FROM generate_series(1, 4) t, generate_series(1, 4) p;
INSERT INTO tariff_periods (id, tariff_id, days, price_rub_minor, stars, sort, active) OVERRIDING SYSTEM VALUE VALUES
    (17, 5, 30, perf_gen.price(5, 1), perf_gen.stars(5, 1), 1, false),
    (18, 6, 30, perf_gen.price(6, 1), perf_gen.stars(6, 1), 1, false);

INSERT INTO tariff_squads (tariff_id, squad_uuid, position)
SELECT t, perf_gen.uuid(1, 1), 0 FROM generate_series(1, 6) t
UNION ALL
SELECT t, perf_gen.uuid(1, 2), 1 FROM generate_series(2, 4) t;

INSERT INTO tariff_channels (tariff_id) SELECT generate_series(1, 6);

CREATE UNLOGGED TABLE perf_gen.c0 AS
SELECT s::bigint AS seed,
       :'t0'::timestamptz - make_interval(secs => (CASE WHEN s % 10007 = 3 THEN 400 + 300 * perf_gen.r(s, 2)
                                                        ELSE 730 * power(perf_gen.r(s, 1), 1.6) END) * 86400 + 600) AS created_at
FROM generate_series(1, :nc) s;

CREATE UNLOGGED TABLE perf_gen.c1 AS
SELECT row_number() OVER (ORDER BY created_at, seed) AS id, seed, created_at,
       perf_gen.r(seed, 3) < 0.96 AS tg,
       perf_gen.r(seed, 5) < 0.012 AS blocked,
       perf_gen.r(seed, 6) < 0.003 AND seed % 10007 <> 3 AS deleted,
       seed % 10007 = 3 AS whale,
       CASE WHEN perf_gen.r(seed, 9) < 0.5 THEN 1 WHEN perf_gen.r(seed, 9) < 0.75 THEN 2 WHEN perf_gen.r(seed, 9) < 0.9 THEN 3 ELSE 4 END AS tariff,
       CASE WHEN seed % 10007 = 3 THEN 1 WHEN perf_gen.r(seed, 10) < 0.7 THEN 1 WHEN perf_gen.r(seed, 10) < 0.88 THEN 2 WHEN perf_gen.r(seed, 10) < 0.95 THEN 3 ELSE 4 END AS pidx,
       created_at + make_interval(secs => (CASE WHEN perf_gen.r(seed, 12) < 0.7 THEN perf_gen.r(seed, 13) * 3 ELSE perf_gen.r(seed, 13) * 40 END) * 86400) AS start_at,
       (seed % 10007 = 3 OR perf_gen.r(seed, 8) < 0.22) AS wants_pay
FROM perf_gen.c0;

CREATE UNLOGGED TABLE perf_gen.c AS
SELECT x.*,
       CASE WHEN paying THEN start_at + make_interval(days => n * days) END AS paid_until
FROM (
    SELECT c1.id, c1.seed, c1.created_at, c1.tg,
           NOT c1.tg OR perf_gen.r(c1.seed, 4) < 0.08 AS email,
           c1.blocked, c1.deleted, c1.whale, c1.tariff, c1.pidx,
           (ARRAY[30, 90, 180, 365])[c1.pidx] AS days,
           (c1.tariff - 1) * 4 + c1.pidx AS period_id,
           c1.start_at,
           c1.wants_pay AND c1.start_at < :'t0'::timestamptz - interval '1 hour' AS paying,
           CASE WHEN c1.whale THEN floor(extract(epoch FROM :'t0'::timestamptz - c1.start_at) / (30 * 86400))::int + 1
                ELSE least(1 + floor(-ln(1 - perf_gen.r(c1.seed, 11) * 0.999999) * 5)::int,
                           floor(extract(epoch FROM :'t0'::timestamptz - c1.start_at) / ((ARRAY[30, 90, 180, 365])[c1.pidx] * 86400))::int + 1,
                           60) END AS n,
           perf_gen.r(c1.seed, 7) < 0.70 AS trial,
           CASE WHEN c1.whale THEN 600
                WHEN c1.wants_pay AND perf_gen.r(c1.seed, 20) < 0.12 THEN 1 + floor(perf_gen.r(c1.seed, 21) * 12)::int
                ELSE 0 END AS topups,
           perf_gen.r(c1.seed, 22) < 0.35 AS autorenew,
           CASE WHEN perf_gen.r(c1.seed, 14) < 0.08 THEN 'en' ELSE 'ru' END AS locale,
           c1.created_at < :'t0'::timestamptz - interval '90 days' AS imported
    FROM perf_gen.c1
) x;
CREATE UNIQUE INDEX ON perf_gen.c (id);
ANALYZE perf_gen.c;

CREATE UNLOGGED TABLE perf_gen.clients AS
SELECT id,
       perf_gen.uuid(seed, 100) AS public_id,
       CASE WHEN blocked THEN 'blocked' ELSE 'active' END AS status,
       CASE WHEN blocked THEN 'нарушение правил' END AS block_reason,
       CASE WHEN blocked THEN created_at + interval '20 days' END AS blocked_at,
       locale,
       CASE WHEN perf_gen.r(seed, 15) < 0.002 THEN 'Постоянный клиент, просил счёт на компанию' END AS note,
       CASE WHEN imported THEN 'migration'
            WHEN NOT tg THEN 'web'
            WHEN perf_gen.r(seed, 16) < 0.85 THEN 'bot'
            ELSE 'miniapp' END AS registration_source,
       tg AND perf_gen.r(seed, 17) < 0.06 AS bot_unreachable,
       created_at,
       created_at + interval '1 minute' AS updated_at,
       CASE WHEN deleted THEN created_at + interval '30 days' END AS deleted_at
FROM perf_gen.c;
CALL perf_gen.load('clients', 'id, public_id, status, block_reason, blocked_at, locale, note, registration_source, bot_unreachable, created_at, updated_at, deleted_at', 'perf_gen.clients');
DROP TABLE perf_gen.clients;

CREATE UNLOGGED TABLE perf_gen.ident AS
SELECT id, client_id, kind, value, verified, meta, created_at, updated_at,
       replace(translate(lower(concat_ws(' ', value, meta ->> 'username', meta ->> 'first_name', meta ->> 'last_name')),
                         'АБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯ', 'абвгдеёжзийклмнопрстуфхцчшщъыьэюя'), 'ё', 'е') AS search_text
FROM (
    SELECT c.id AS id, c.id AS client_id, 'telegram' AS kind, (5000000000 + c.seed * 13)::text AS value, true AS verified,
           jsonb_strip_nulls(jsonb_build_object(
               'username', CASE WHEN perf_gen.r(c.seed, 30) < 0.62 THEN 'tg_' || to_hex((c.seed * 2654435761) % 4294967296) END,
               'first_name', perf_gen.pick(ARRAY['Алексей', 'Дмитрий', 'Иван', 'Сергей', 'Андрей', 'Михаил', 'Никита', 'Артём', 'Егор', 'Максим',
                                                 'Анна', 'Мария', 'Елена', 'Ольга', 'Наталья', 'Татьяна', 'Юлия', 'Алёна', 'Ксения', 'Дарья',
                                                 'Alex', 'John', 'Kate', 'Max', 'Олег', 'Павел', 'Роман', 'Ирина', 'Светлана', 'Виктор'], perf_gen.r(c.seed, 31)),
               'last_name', CASE WHEN perf_gen.r(c.seed, 32) < 0.4 THEN perf_gen.pick(ARRAY['Иванов', 'Смирнов', 'Кузнецов', 'Попов', 'Васильев',
                                                 'Петров', 'Соколов', 'Михайлов', 'Новиков', 'Фёдоров', 'Морозов', 'Волков', 'Алексеев', 'Лебедев',
                                                 'Семёнов', 'Егоров', 'Павлов', 'Козлов', 'Степанов', 'Николаев'], perf_gen.r(c.seed, 33)) END,
               'language_code', CASE WHEN c.locale = 'en' THEN 'en' ELSE 'ru' END)) AS meta,
           c.created_at, c.created_at AS updated_at
    FROM perf_gen.c c WHERE c.tg
    UNION ALL
    SELECT :nc + c.id, c.id, 'email', 'mail' || c.seed || '@' || perf_gen.pick(ARRAY['gmail.com', 'yandex.ru', 'mail.ru', 'outlook.com', 'proton.me'], perf_gen.r(c.seed, 34)),
           perf_gen.r(c.seed, 35) < 0.9, '{}'::jsonb, c.created_at + interval '2 minutes', c.created_at + interval '2 minutes'
    FROM perf_gen.c c WHERE c.email
) x;
CALL perf_gen.load('client_identities', 'id, client_id, kind, value, verified, meta, created_at, updated_at, search_text', 'perf_gen.ident');
DROP TABLE perf_gen.ident;

CREATE UNLOGGED TABLE perf_gen.sub0 AS
SELECT c.id AS client_id, 'main' AS role, c.seed,
       CASE WHEN c.paying THEN 'paid' WHEN c.trial THEN 'trial' WHEN perf_gen.r(c.seed, 24) < 0.20 THEN 'promo'
            WHEN perf_gen.r(c.seed, 27) < 0.02 THEN 'gift' WHEN perf_gen.r(c.seed, 28) < 0.01 THEN 'manual' END AS kind,
       CASE WHEN c.paying OR perf_gen.r(c.seed, 27) < 0.02 OR perf_gen.r(c.seed, 28) < 0.01 THEN c.tariff END AS tariff_id,
       CASE WHEN c.paying AND c.trial THEN c.created_at + interval '3 minutes'
            WHEN c.paying THEN c.start_at
            ELSE c.created_at + interval '3 minutes' END AS created_at,
       CASE WHEN c.paying THEN c.paid_until
            WHEN c.trial THEN c.created_at + interval '3 days'
            WHEN perf_gen.r(c.seed, 24) < 0.20 THEN c.created_at + interval '7 days'
            ELSE c.created_at + interval '30 days' END AS expire_at,
       c.paying AND c.autorenew AS autorenew,
       c.blocked
FROM perf_gen.c c
WHERE c.paying OR c.trial OR perf_gen.r(c.seed, 24) < 0.20 OR perf_gen.r(c.seed, 27) < 0.02 OR perf_gen.r(c.seed, 28) < 0.01
UNION ALL
SELECT c.id, 'hist', c.seed + 1000000000, 'paid', 5 + (c.seed % 2)::int, c.created_at - interval '1 minute',
       c.start_at - interval '1 day', false, c.blocked
FROM perf_gen.c c WHERE c.paying AND c.imported AND perf_gen.r(c.seed, 25) < 0.35
UNION ALL
SELECT c.id, 'second', c.seed + 2000000000, 'paid', 1 + (c.seed % 4)::int, c.start_at + interval '10 days',
       greatest(c.paid_until, c.start_at + interval '40 days'), false, c.blocked
FROM perf_gen.c c WHERE c.paying AND perf_gen.r(c.seed, 26) < 0.05 AND c.start_at + interval '10 days' < :'t0'::timestamptz;

CREATE UNLOGGED TABLE perf_gen.sub AS
SELECT row_number() OVER (ORDER BY created_at, client_id, role) AS id, s.*,
       CASE WHEN s.role = 'hist' THEN s.expire_at END AS ended_at,
       CASE WHEN s.role = 'hist' THEN 'expired'
            WHEN s.blocked THEN 'disabled'
            WHEN s.expire_at > :'t0'::timestamptz AND perf_gen.r(s.seed, 23) < 0.03 THEN 'limited'
            WHEN s.expire_at > :'t0'::timestamptz THEN 'active'
            ELSE 'expired' END AS status
FROM perf_gen.sub0 s;
CREATE INDEX ON perf_gen.sub (client_id, role);
ANALYZE perf_gen.sub;
DROP TABLE perf_gen.sub0;

CREATE UNLOGGED TABLE perf_gen.subs AS
SELECT s.id,
       perf_gen.uuid(s.seed, 200) AS public_id,
       s.client_id, s.tariff_id, s.kind,
       s.id AS remna_user_id,
       'u' || s.id AS remna_username,
       substr(md5(s.id::text || 'short'), 1, 16) AS short_uuid,
       CASE WHEN s.kind = 'paid' AND perf_gen.r(s.seed, 40) < 0.05 THEN 1 ELSE 0 END AS extra_devices,
       s.created_at AS period_started_at,
       CASE WHEN s.kind = 'trial' THEN 3 WHEN s.kind = 'promo' THEN 7 ELSE 30 END AS period_days,
       s.autorenew AS autorenew_enabled,
       CASE WHEN s.autorenew THEN c.period_id END AS autorenew_period_id,
       s.status, s.expire_at,
       (perf_gen.r(s.seed, 41) * 80000000000)::bigint AS traffic_used_bytes,
       CASE WHEN s.tariff_id = 4 THEN 536870912000 END AS traffic_limit_bytes,
       CASE WHEN s.tariff_id = 4 THEN 'MONTH' ELSE 'NO_RESET' END AS traffic_strategy,
       CASE s.tariff_id WHEN 1 THEN 1 WHEN 2 THEN 3 WHEN 3 THEN 5 WHEN 4 THEN 10 ELSE 2 END AS hwid_limit,
       floor(perf_gen.r(s.seed, 42) * 3)::int AS devices_count,
       CASE WHEN perf_gen.r(s.seed, 43) < 0.82 THEN least(:'t0'::timestamptz, s.expire_at) - make_interval(secs => perf_gen.r(s.seed, 44) * 86400 * 5) END AS online_at,
       CASE WHEN perf_gen.r(s.seed, 43) < 0.82 THEN s.created_at + make_interval(secs => perf_gen.r(s.seed, 45) * 7200) END AS first_connected_at,
       perf_gen.uuid(1 + floor(perf_gen.r(s.seed, 46) * 12)::bigint, 300) AS last_node_uuid,
       :'t0'::timestamptz - make_interval(secs => perf_gen.r(s.seed, 47) * 3600) AS remna_updated_at,
       :'t0'::timestamptz - make_interval(secs => perf_gen.r(s.seed, 48) * 3600) AS synced_at,
       s.created_at,
       greatest(s.created_at, least(:'t0'::timestamptz, s.expire_at)) AS updated_at,
       s.ended_at
FROM perf_gen.sub s JOIN perf_gen.c c ON c.id = s.client_id;
CALL perf_gen.load('subscriptions', 'id, public_id, client_id, tariff_id, kind, remna_user_id, remna_username, short_uuid, extra_devices, period_started_at, period_days, autorenew_enabled, autorenew_period_id, status, expire_at, traffic_used_bytes, traffic_limit_bytes, traffic_strategy, hwid_limit, devices_count, online_at, first_connected_at, last_node_uuid, remna_updated_at, synced_at, created_at, updated_at, ended_at', 'perf_gen.subs');
DROP TABLE perf_gen.subs;

CREATE UNLOGGED TABLE perf_gen.ord0 (
    oseed bigint NOT NULL,
    client_id bigint NOT NULL,
    subscription_id bigint,
    cls text NOT NULL,
    kind text NOT NULL,
    status text NOT NULL,
    tariff integer,
    pidx integer,
    total_minor bigint NOT NULL,
    created_at timestamptz NOT NULL,
    paid_at timestamptz,
    provider text,
    attempts integer NOT NULL,
    ref text
);

INSERT INTO perf_gen.ord0
SELECT c.seed * 10000 + k, c.id, s.id, 'sub', CASE WHEN k = 0 THEN 'new' ELSE 'renewal' END,
       CASE WHEN perf_gen.r(c.seed * 10000 + k, 50) < 0.015 THEN 'refunded' ELSE 'fulfilled' END,
       c.tariff, c.pidx, perf_gen.price(c.tariff, c.pidx),
       p.paid_at - interval '12 minutes', p.paid_at,
       CASE WHEN perf_gen.r(c.seed * 10000 + k, 51) < 0.07 THEN 'stars'
            WHEN p.paid_at < :'t0'::timestamptz - interval '90 days'
                THEN perf_gen.pick(ARRAY['yookassa', 'yookassa', 'yoomoney', 'cryptobot', 'platega', 'lava'], perf_gen.r(c.seed * 10000 + k, 52))
            ELSE perf_gen.pick(ARRAY['yookassa', 'yookassa', 'platega', 'platega', 'cryptopay', 'heleket', 'overpay', 'cloudpayments', 'lava'], perf_gen.r(c.seed * 10000 + k, 52)) END,
       1 + (perf_gen.r(c.seed * 10000 + k, 53) < 0.65)::int + (perf_gen.r(c.seed * 10000 + k, 55) < 0.30)::int + (perf_gen.r(c.seed * 10000 + k, 56) < 0.10)::int,
       NULL
FROM perf_gen.c c
JOIN perf_gen.sub s ON s.client_id = c.id AND s.role = 'main'
CROSS JOIN LATERAL generate_series(0, c.n - 1) k
CROSS JOIN LATERAL (SELECT least(c.start_at + make_interval(days => k * c.days) + make_interval(secs => perf_gen.r(c.seed * 10000 + k, 54) * 21600),
                                 :'t0'::timestamptz - interval '2 minutes') AS paid_at) p
WHERE c.paying;

INSERT INTO perf_gen.ord0
SELECT c.seed * 10000 + 100 + j, c.id, s.id, 'abandoned', CASE WHEN c.paying THEN 'renewal' ELSE 'new' END,
       CASE WHEN o.created_at > :'t0'::timestamptz - interval '1 hour' THEN 'pending' ELSE 'canceled' END,
       c.tariff, c.pidx, perf_gen.price(c.tariff, c.pidx), o.created_at, NULL,
       CASE WHEN perf_gen.r(c.seed * 10000 + 100 + j, 51) < 0.07 THEN 'stars'
            WHEN o.created_at < :'t0'::timestamptz - interval '90 days'
                THEN perf_gen.pick(ARRAY['yookassa', 'yoomoney', 'cryptobot', 'platega', 'lava'], perf_gen.r(c.seed * 10000 + 100 + j, 52))
            ELSE perf_gen.pick(ARRAY['yookassa', 'platega', 'platega', 'cryptopay', 'heleket', 'overpay', 'cloudpayments', 'lava'], perf_gen.r(c.seed * 10000 + 100 + j, 52)) END,
       1 + (perf_gen.r(c.seed * 10000 + 100 + j, 53) < 0.65)::int + (perf_gen.r(c.seed * 10000 + 100 + j, 55) < 0.30)::int + (perf_gen.r(c.seed * 10000 + 100 + j, 56) < 0.10)::int,
       NULL
FROM perf_gen.c c
LEFT JOIN perf_gen.sub s ON s.client_id = c.id AND s.role = 'main' AND c.paying
CROSS JOIN LATERAL generate_series(1, CASE WHEN perf_gen.r(c.seed, 60) < 0.75 THEN 1 + floor(perf_gen.r(c.seed, 61) * 2.6)::int ELSE 0 END) j
CROSS JOIN LATERAL (SELECT c.created_at + (:'t0'::timestamptz - c.created_at) * perf_gen.r(c.seed * 10000 + 100 + j, 62) AS created_at) o
WHERE NOT c.deleted;

INSERT INTO perf_gen.ord0
SELECT c.seed * 10000 + 1000 + j, c.id, NULL, 'topup', 'topup', 'fulfilled', NULL, NULL,
       perf_gen.price(c.tariff, 1) * (1 + floor(perf_gen.r(c.seed * 10000 + 1000 + j, 63) * 3)),
       t.at, t.at + interval '6 minutes',
       CASE WHEN t.at < :'t0'::timestamptz - interval '90 days'
            THEN perf_gen.pick(ARRAY['yookassa', 'yoomoney', 'cryptobot'], perf_gen.r(c.seed * 10000 + 1000 + j, 52))
            ELSE perf_gen.pick(ARRAY['yookassa', 'platega', 'cryptopay', 'heleket'], perf_gen.r(c.seed * 10000 + 1000 + j, 52)) END,
       CASE WHEN perf_gen.r(c.seed * 10000 + 1000 + j, 53) < 0.1 THEN 2 ELSE 1 END,
       NULL
FROM perf_gen.c c
CROSS JOIN LATERAL generate_series(1, c.topups) j
CROSS JOIN LATERAL (SELECT c.created_at + (:'t0'::timestamptz - interval '30 minutes' - c.created_at) * ((j - 1 + perf_gen.r(c.seed * 10000 + 1000 + j, 64) * 0.5) / c.topups) AS at) t
WHERE c.topups > 0;

INSERT INTO perf_gen.ord0
SELECT o.oseed + 1000, o.client_id, s.id, 'balance', 'renewal', 'fulfilled', c.tariff, 1, perf_gen.price(c.tariff, 1),
       o.paid_at + interval '3 minutes', o.paid_at + interval '3 minutes', 'balance', 1, NULL
FROM perf_gen.ord0 o
JOIN perf_gen.c c ON c.id = o.client_id
LEFT JOIN perf_gen.sub s ON s.client_id = c.id AND s.role = 'main'
WHERE o.cls = 'topup' AND perf_gen.r(o.oseed, 65) < 0.7;

INSERT INTO perf_gen.ord0
SELECT c.seed * 10000 + 3000, c.id, s.id, 'trial', 'trial', 'fulfilled', NULL, NULL, 0, s0.at, s0.at, NULL, 0, NULL
FROM perf_gen.c c
JOIN perf_gen.sub s ON s.client_id = c.id AND s.role = 'main'
CROSS JOIN LATERAL (SELECT c.created_at + interval '3 minutes' AS at) s0
WHERE c.trial;

INSERT INTO perf_gen.ord0
SELECT c.seed * 10000 + 3001, c.id, s.id, 'grant', 'module_grant', 'fulfilled', NULL, NULL, 0, s.created_at, s.created_at, NULL, 0,
       'P' || lpad((1 + c.seed % 40)::text, 4, '0')
FROM perf_gen.c c
JOIN perf_gen.sub s ON s.client_id = c.id AND s.role = 'main' AND s.kind = 'promo';

INSERT INTO perf_gen.ord0
SELECT c.seed * 10000 + 3002, c.id, s.id, 'admin', 'admin_grant', 'fulfilled', s.tariff_id, NULL, 0, s.created_at, s.created_at, NULL, 0, NULL
FROM perf_gen.c c
JOIN perf_gen.sub s ON s.client_id = c.id AND s.role = 'main' AND s.kind = 'manual';

INSERT INTO perf_gen.ord0
SELECT g.seed * 10000 + 3003, g.id, NULL, 'gift_buy', 'module', 'fulfilled', 2, 1, perf_gen.price(2, 1),
       g.created_at + interval '1 day', g.created_at + interval '1 day' + interval '5 minutes',
       perf_gen.pick(ARRAY['yookassa', 'platega', 'cryptopay'], perf_gen.r(g.seed, 66)), 1, NULL
FROM perf_gen.c g
WHERE g.paying AND perf_gen.r(g.seed, 67) < 0.08;

INSERT INTO perf_gen.ord0
SELECT c.seed * 10000 + 3004, c.id, s.id, 'gift_grant', 'module_grant', 'fulfilled', s.tariff_id, NULL, 0, s.created_at, s.created_at, NULL, 0, NULL
FROM perf_gen.c c
JOIN perf_gen.sub s ON s.client_id = c.id AND s.role = 'main' AND s.kind = 'gift';

CREATE UNLOGGED TABLE perf_gen.ord AS
SELECT row_number() OVER (ORDER BY created_at, oseed) AS id, o.*
FROM perf_gen.ord0 o;
CREATE INDEX ON perf_gen.ord (oseed);
ANALYZE perf_gen.ord;
DROP TABLE perf_gen.ord0;

CREATE UNLOGGED TABLE perf_gen.orders AS
SELECT o.id,
       perf_gen.uuid(o.oseed, 300) AS public_id,
       o.client_id, o.subscription_id, o.kind, o.status,
       CASE
           WHEN o.cls IN ('sub', 'abandoned', 'balance') AND o.created_at < :'t0'::timestamptz - interval '90 days' THEN
               jsonb_build_object('type', 'imported', 'source', 'stealthnet', 'old_id', o.oseed::text,
                                  'label', (ARRAY['Старт', 'Стандарт', 'Семья', 'Про'])[o.tariff] || ' · ' || (ARRAY[30, 90, 180, 365])[o.pidx] || ' дней')
           WHEN o.cls IN ('sub', 'abandoned', 'balance') THEN
               jsonb_build_object('type', 'subscription', 'target', CASE WHEN o.kind = 'renewal' THEN o.subscription_id END,
                                  'period_id', (o.tariff - 1) * 4 + o.pidx, 'tariff_id', o.tariff, 'extra_devices', 0)
           WHEN o.cls = 'topup' THEN jsonb_build_object('type', 'top_up', 'amount', perf_gen.money(o.total_minor))
           WHEN o.cls = 'trial' THEN jsonb_build_object('type', 'trial')
           WHEN o.cls = 'grant' THEN jsonb_build_object('type', 'module_grant', 'module', 'promolinks', 'reference', o.ref,
                                                        'tariff_id', NULL, 'period_id', NULL, 'days', 7)
           WHEN o.cls = 'gift_grant' THEN jsonb_build_object('type', 'module_grant', 'module', 'gifts', 'reference', 'GIFT-' || o.oseed,
                                                             'tariff_id', o.tariff, 'period_id', NULL, 'days', 30)
           WHEN o.cls = 'gift_buy' THEN jsonb_build_object('type', 'module', 'module', 'gifts', 'kind', 'gift',
                                                           'payload', jsonb_build_object('tariff_id', 2, 'period_id', 5))
           ELSE jsonb_build_object('type', 'admin_grant', 'tariff_id', o.tariff, 'period_id', NULL, 'days', 30, 'note', NULL)
       END AS intent,
       CASE
           WHEN o.cls IN ('sub', 'abandoned', 'balance') AND o.created_at < :'t0'::timestamptz - interval '90 days' THEN
               jsonb_build_object('days', (ARRAY[30, 90, 180, 365])[o.pidx], 'lines', '[]'::jsonb,
                                  'stars', CASE WHEN o.provider = 'stars' THEN jsonb_build_object('subtotal', perf_gen.stars(o.tariff, o.pidx), 'discount', 0, 'total', perf_gen.stars(o.tariff, o.pidx)) END,
                                  'total', perf_gen.money(CASE WHEN o.provider = 'stars' THEN 0 ELSE o.total_minor END),
                                  'currency', 'RUB', 'discount', perf_gen.money(0),
                                  'subtotal', perf_gen.money(CASE WHEN o.provider = 'stars' THEN 0 ELSE o.total_minor END), 'extra_devices', 0)
           WHEN o.cls IN ('sub', 'abandoned', 'balance') THEN
               jsonb_build_object('days', (ARRAY[30, 90, 180, 365])[o.pidx],
                                  'lines', jsonb_build_array(jsonb_build_object(
                                      'kind', jsonb_build_object('days', (ARRAY[30, 90, 180, 365])[o.pidx], 'type', 'period',
                                                                 'months', (ARRAY[1, 3, 6, 12])[o.pidx], 'period_id', (o.tariff - 1) * 4 + o.pidx, 'tariff_id', o.tariff),
                                      'label', (ARRAY['Старт', 'Стандарт', 'Семья', 'Про'])[o.tariff] || ' · ' || (ARRAY[30, 90, 180, 365])[o.pidx] || ' дней',
                                      'stars', perf_gen.stars(o.tariff, o.pidx), 'amount', perf_gen.money(o.total_minor))),
                                  'stars', jsonb_build_object('subtotal', perf_gen.stars(o.tariff, o.pidx), 'discount', 0, 'total', perf_gen.stars(o.tariff, o.pidx)),
                                  'total', perf_gen.money(o.total_minor), 'currency', 'RUB', 'discount', perf_gen.money(0),
                                  'subtotal', perf_gen.money(o.total_minor), 'extra_devices', 0)
           WHEN o.cls = 'topup' THEN
               jsonb_build_object('days', 0, 'lines', jsonb_build_array(jsonb_build_object('kind', jsonb_build_object('type', 'top_up'),
                                  'label', 'Пополнение баланса', 'stars', NULL, 'amount', perf_gen.money(o.total_minor))),
                                  'stars', NULL, 'total', perf_gen.money(o.total_minor), 'currency', 'RUB', 'discount', perf_gen.money(0),
                                  'subtotal', perf_gen.money(o.total_minor), 'extra_devices', 0)
           ELSE jsonb_build_object('days', CASE o.cls WHEN 'trial' THEN 3 WHEN 'grant' THEN 7 ELSE 30 END, 'lines', '[]'::jsonb, 'stars', NULL,
                                   'total', perf_gen.money(o.total_minor), 'currency', 'RUB', 'discount', perf_gen.money(0),
                                   'subtotal', perf_gen.money(o.total_minor), 'extra_devices', 0)
       END AS quote,
       CASE WHEN o.cls IN ('sub', 'abandoned', 'balance') AND o.created_at >= :'t0'::timestamptz - interval '90 days' THEN
           jsonb_build_object('days', (ARRAY[30, 90, 180, 365])[o.pidx], 'name', (ARRAY['Старт', 'Стандарт', 'Семья', 'Про'])[o.tariff],
                              'squads', jsonb_build_array(perf_gen.uuid(1, 1)), 'traffic', jsonb_build_object('reset', 'no_reset', 'limit_bytes', NULL),
                              'period_id', (o.tariff - 1) * 4 + o.pidx, 'tariff_id', o.tariff, 'category_id', 1, 'external_squad', NULL,
                              'devices_included', (ARRAY[1, 3, 5, 10])[o.tariff], 'max_extra_devices', 2)
       END AS snapshot,
       'RUB' AS currency,
       CASE WHEN o.provider = 'stars' AND o.created_at < :'t0'::timestamptz - interval '90 days' THEN 0 ELSE o.total_minor END AS total_minor,
       CASE WHEN o.provider = 'stars' THEN perf_gen.stars(o.tariff, o.pidx) END AS stars_total,
       o.created_at,
       COALESCE(o.paid_at + interval '6 seconds', CASE WHEN o.status = 'canceled' THEN o.created_at + interval '1 hour' ELSE o.created_at END) AS updated_at,
       o.paid_at,
       CASE WHEN o.status IN ('fulfilled', 'refunded') THEN o.paid_at + interval '5 seconds' END AS fulfilled_at,
       CASE WHEN o.status = 'canceled' THEN o.created_at + interval '1 hour' END AS canceled_at
FROM perf_gen.ord o;
CALL perf_gen.load('orders', 'id, public_id, client_id, subscription_id, kind, status, intent, quote, snapshot, currency, total_minor, stars_total, created_at, updated_at, paid_at, fulfilled_at, canceled_at', 'perf_gen.orders');
DROP TABLE perf_gen.orders;

CREATE UNLOGGED TABLE perf_gen.pay0 AS
SELECT o.id AS order_id, o.oseed * 4 + a AS pseed, o.client_id, o.cls, o.provider, o.status AS order_status,
       o.tariff, o.pidx, o.total_minor,
       CASE WHEN a = o.attempts AND o.paid_at IS NOT NULL THEN 'main' ELSE 'attempt' END AS role,
       o.created_at + make_interval(secs => 60 * a) AS created_at,
       o.paid_at,
       a = o.attempts AS last
FROM perf_gen.ord o
CROSS JOIN LATERAL generate_series(1, o.attempts) a
WHERE o.provider IS NOT NULL;

CREATE UNLOGGED TABLE perf_gen.pay AS
SELECT row_number() OVER (ORDER BY created_at, pseed) AS id, p.*
FROM perf_gen.pay0 p;
ANALYZE perf_gen.pay;
DROP TABLE perf_gen.pay0;

CREATE UNLOGGED TABLE perf_gen.payments AS
SELECT p.id,
       perf_gen.uuid(p.pseed, 400) AS public_id,
       p.order_id, p.client_id, p.provider,
       CASE p.provider WHEN 'yookassa' THEN perf_gen.pick(ARRAY['bank_card', 'sbp', 'sberbank'], perf_gen.r(p.pseed, 70))
                       WHEN 'platega' THEN 'sbp' WHEN 'stars' THEN NULL WHEN 'balance' THEN NULL ELSE 'crypto' END AS method,
       CASE WHEN p.provider = 'stars' THEN perf_gen.stars(p.tariff, p.pidx) ELSE p.total_minor END AS amount_minor,
       CASE WHEN p.provider = 'stars' THEN 'XTR' ELSE 'RUB' END AS currency,
       CASE WHEN p.role = 'main' AND p.order_status = 'refunded' THEN 'refunded'
            WHEN p.role = 'main' AND perf_gen.r(p.pseed, 71) < 0.004 THEN 'partially_refunded'
            WHEN p.role = 'main' THEN 'paid'
            WHEN p.order_status = 'pending' AND p.last THEN 'pending'
            WHEN perf_gen.r(p.pseed, 72) < 0.55 THEN 'canceled'
            ELSE 'failed' END AS status,
       CASE WHEN p.provider = 'balance' THEN NULL
            WHEN p.role = 'main' OR perf_gen.r(p.pseed, 72) >= 0.55 THEN p.provider || '-' || p.id || '-' || substr(md5(p.pseed::text), 1, 10) END AS provider_txid,
       CASE WHEN p.provider NOT IN ('balance', 'stars') AND p.created_at >= :'t0'::timestamptz - interval '90 days'
            THEN 'https://pay.example.com/i/' || substr(md5(p.pseed::text || 'u'), 1, 24) END AS pay_url,
       CASE WHEN p.created_at < :'t0'::timestamptz - interval '90 days'
                THEN jsonb_build_object('imported', jsonb_build_object('source', 'stealthnet', 'old_id', p.pseed::text,
                                        'external_id', substr(md5(p.pseed::text || 'e'), 1, 20), 'provider', p.provider))
            WHEN p.provider = 'balance' THEN '{}'::jsonb
            ELSE jsonb_build_object('status', 'succeeded', 'test', false, 'id', substr(md5(p.pseed::text || 'e'), 1, 20)) END AS provider_payload,
       CASE WHEN p.role <> 'main' AND perf_gen.r(p.pseed, 72) >= 0.55 THEN perf_gen.pick(ARRAY['Недостаточно средств', 'Карта отклонена банком', 'Платёж отменён плательщиком'], perf_gen.r(p.pseed, 73)) END AS failure_reason,
       CASE WHEN p.role = 'main' AND p.order_status = 'refunded' THEN CASE WHEN p.provider = 'stars' THEN perf_gen.stars(p.tariff, p.pidx) ELSE p.total_minor END
            WHEN p.role = 'main' AND perf_gen.r(p.pseed, 71) < 0.004 THEN greatest(1, p.total_minor / 2)
            ELSE 0 END AS refunded_minor,
       CASE WHEN p.provider NOT IN ('balance', 'stars') AND p.created_at >= :'t0'::timestamptz - interval '90 days' THEN p.created_at + interval '30 minutes' END AS expires_at,
       CASE WHEN p.role = 'main' THEN p.paid_at END AS paid_at,
       CASE WHEN p.created_at >= :'t0'::timestamptz - interval '90 days' AND p.provider IN ('yookassa', 'platega') THEN perf_gen.pick(ARRAY['card', 'sbp'], perf_gen.r(p.pseed, 74)) END AS pay_method,
       p.created_at,
       COALESCE(CASE WHEN p.role = 'main' THEN p.paid_at END, p.created_at + interval '30 minutes') AS updated_at
FROM perf_gen.pay p;
CALL perf_gen.load('payments', 'id, public_id, order_id, client_id, provider, method, amount_minor, currency, status, provider_txid, pay_url, provider_payload, failure_reason, refunded_minor, expires_at, paid_at, pay_method, created_at, updated_at', 'perf_gen.payments');
CREATE UNLOGGED TABLE perf_gen.paid AS
SELECT id, order_id, client_id, provider, amount_minor, currency, status, refunded_minor, paid_at, created_at
FROM perf_gen.payments WHERE status IN ('paid', 'refunded', 'partially_refunded');
CREATE INDEX ON perf_gen.paid (order_id);
CREATE INDEX ON perf_gen.paid (client_id);
ANALYZE perf_gen.paid;
CREATE UNLOGGED TABLE perf_gen.payrecent AS
SELECT id, order_id, client_id, provider, amount_minor, currency, status, created_at, paid_at, provider_txid, public_id
FROM perf_gen.payments WHERE created_at >= :'t0'::timestamptz - interval '30 days' AND provider NOT IN ('balance', 'stars');
DROP TABLE perf_gen.payments;

INSERT INTO refunds (payment_id, amount_minor, currency, method, subscription_action, reason, created_at)
SELECT p.id, p.refunded_minor, p.currency, CASE WHEN perf_gen.r(p.id, 75) < 0.3 THEN 'balance' ELSE 'provider' END,
       CASE WHEN p.status = 'refunded' THEN 'cut' ELSE 'keep' END, 'Возврат по просьбе клиента', p.paid_at + interval '2 days'
FROM perf_gen.paid p WHERE p.refunded_minor > 0 ORDER BY p.id;

INSERT INTO provider_refunds (payment_id, amount_minor, currency, subscription_action, reason, provider_refund_id, status, created_at, resolved_at)
SELECT p.id, p.refunded_minor, p.currency, 'keep', 'Возврат по просьбе клиента', 'rf-' || p.id,
       CASE WHEN perf_gen.r(p.id, 76) < 0.9 THEN 'succeeded' ELSE 'declined' END, p.paid_at + interval '2 days', p.paid_at + interval '2 days 1 hour'
FROM perf_gen.paid p WHERE p.refunded_minor > 0 AND p.provider NOT IN ('balance', 'stars') AND perf_gen.r(p.id, 77) < 0.3 ORDER BY p.id;

CREATE UNLOGGED TABLE perf_gen.ledger0 AS
SELECT p.client_id, p.created_at, p.id AS payment_id, o.id AS order_id, 'topup' AS kind, p.amount_minor AS delta, 'main' AS wallet
FROM perf_gen.paid p JOIN perf_gen.ord o ON o.id = p.order_id WHERE o.cls = 'topup' AND p.status = 'paid'
UNION ALL
SELECT p.client_id, p.created_at, p.id, o.id, 'purchase', -p.amount_minor, 'main'
FROM perf_gen.paid p JOIN perf_gen.ord o ON o.id = p.order_id WHERE o.cls = 'balance';

CREATE UNLOGGED TABLE perf_gen.ledger AS
SELECT row_number() OVER (ORDER BY created_at, payment_id, kind) AS id, l.*,
       sum(delta) OVER (PARTITION BY client_id, wallet ORDER BY created_at, payment_id, kind ROWS UNBOUNDED PRECEDING) AS balance_after
FROM perf_gen.ledger0 l;
DROP TABLE perf_gen.ledger0;
DELETE FROM perf_gen.ledger l WHERE l.balance_after < 0;

CREATE UNLOGGED TABLE perf_gen.balance_entries AS
SELECT id, client_id, wallet, 'RUB' AS currency, delta AS delta_minor, balance_after AS balance_after_minor, kind, order_id, payment_id,
       CASE WHEN kind = 'topup' THEN 'topup:' || payment_id ELSE 'purchase:' || order_id END AS idempotency_key, created_at
FROM perf_gen.ledger;
CALL perf_gen.load('balance_entries', 'id, client_id, wallet, currency, delta_minor, balance_after_minor, kind, order_id, payment_id, idempotency_key, created_at', 'perf_gen.balance_entries');
DROP TABLE perf_gen.balance_entries;

INSERT INTO client_balances (client_id, wallet, currency, amount_minor, updated_at)
SELECT DISTINCT ON (client_id) client_id, wallet, 'RUB', balance_after, created_at
FROM perf_gen.ledger ORDER BY client_id, created_at DESC, id DESC;

INSERT INTO trial_claims (kind, value, client_id, claimed_at)
SELECT 'telegram', (5000000000 + c.seed * 13)::text, c.id, c.created_at + interval '3 minutes'
FROM perf_gen.c c WHERE c.trial AND c.tg;

INSERT INTO client_utm (client_id, source, medium, campaign, start_param, registration_source, created_at)
SELECT c.id, perf_gen.pick(ARRAY['telegram', 'youtube', 'vk', 'tiktok', 'blog'], perf_gen.r(c.seed, 80)),
       perf_gen.pick(ARRAY['cpc', 'post', 'story', 'ads'], perf_gen.r(c.seed, 81)),
       'camp' || (1 + floor(perf_gen.r(c.seed, 82) * 25)::int), 'utm_' || c.seed,
       CASE WHEN c.tg THEN 'bot' ELSE 'web' END, c.created_at
FROM perf_gen.c c WHERE perf_gen.r(c.seed, 83) < 0.3 ORDER BY c.id;

INSERT INTO referral_codes (client_id, code, percent, created_at)
SELECT c.id, 'R' || upper(to_hex(c.seed * 7 + 1048576)), NULL, c.created_at + interval '1 hour'
FROM perf_gen.c c WHERE perf_gen.r(c.seed, 84) < 0.25 ORDER BY c.id;

CREATE UNLOGGED TABLE perf_gen.ref AS
SELECT c.id AS client_id,
       CASE WHEN perf_gen.r(c.seed, 86) < 0.3 THEN 1 + floor(perf_gen.r(c.seed, 87) * least(c.id - 1, 2000))::bigint
            ELSE 1 + floor(perf_gen.r(c.seed, 87) * (c.id - 1))::bigint END AS referrer_id,
       c.created_at
FROM perf_gen.c c WHERE c.id > 1 AND perf_gen.r(c.seed, 85) < 0.15;
INSERT INTO referral_links (client_id, referrer_id, created_at)
SELECT client_id, referrer_id, created_at FROM perf_gen.ref ORDER BY client_id;

INSERT INTO referral_payments (payment_id, processed_at)
SELECT p.id, p.paid_at + interval '10 seconds'
FROM perf_gen.paid p JOIN perf_gen.ref r ON r.client_id = p.client_id
WHERE p.provider <> 'balance' AND p.currency = 'RUB' ORDER BY p.id;

INSERT INTO referral_credits (payment_id, level, referrer_id, referral_id, base_minor, percent, amount_minor, currency, credited_at,
                              reversal_target_minor, reversed_minor, created_at, updated_at)
SELECT p.id, 1, r.referrer_id, p.client_id, p.amount_minor, 10, greatest(1, p.amount_minor / 10), 'RUB', p.paid_at + interval '10 seconds',
       CASE WHEN p.status = 'refunded' THEN greatest(1, p.amount_minor / 10) ELSE 0 END,
       CASE WHEN p.status = 'refunded' THEN greatest(1, p.amount_minor / 10) ELSE 0 END,
       p.paid_at + interval '10 seconds', p.paid_at + interval '10 seconds'
FROM perf_gen.paid p JOIN perf_gen.ref r ON r.client_id = p.client_id
WHERE p.provider <> 'balance' AND p.currency = 'RUB' AND p.amount_minor >= 10 ORDER BY p.id;

INSERT INTO balance_entries (client_id, wallet, currency, delta_minor, balance_after_minor, kind, payment_id, idempotency_key, created_at)
SELECT referrer_id, 'referral', 'RUB', amount_minor,
       sum(amount_minor) OVER (PARTITION BY referrer_id ORDER BY id ROWS UNBOUNDED PRECEDING), 'referral_credit', payment_id,
       'referral:' || payment_id || ':1', credited_at
FROM referral_credits WHERE reversed_minor = 0 ORDER BY id;

INSERT INTO client_balances (client_id, wallet, currency, amount_minor, updated_at)
SELECT referrer_id, 'referral', 'RUB', sum(amount_minor), max(credited_at)
FROM referral_credits WHERE reversed_minor = 0 GROUP BY referrer_id;

INSERT INTO referral_withdrawals (client_id, amount_minor, currency, method, details, status, admin_id, held_at, released_at, created_at, updated_at, decided_at)
SELECT b.client_id, least(b.amount_minor, 300000), 'RUB', perf_gen.pick(ARRAY['sbp', 'card', 'usdt_trc20'], perf_gen.r(b.client_id, 90)),
       '+7 900 000-00-00',
       CASE WHEN perf_gen.r(b.client_id, 91) < 0.1 THEN 'pending' WHEN perf_gen.r(b.client_id, 91) < 0.85 THEN 'paid' ELSE 'rejected' END,
       NULL, b.updated_at, CASE WHEN perf_gen.r(b.client_id, 91) >= 0.85 THEN b.updated_at + interval '1 day' END,
       b.updated_at, b.updated_at, CASE WHEN perf_gen.r(b.client_id, 91) >= 0.1 THEN b.updated_at + interval '1 day' END
FROM client_balances b WHERE b.wallet = 'referral' AND b.amount_minor > 50000 ORDER BY b.client_id;

INSERT INTO promo_codes (code, kind, percent, amount_minor, currency, days, max_uses, per_client, valid_from, valid_until, first_purchase_only, active, note, created_at, updated_at)
SELECT 'PROMO' || lpad(i::text, 5, '0'),
       (ARRAY['percent', 'fixed', 'days'])[1 + i % 3],
       CASE WHEN i % 3 = 0 THEN 5 + i % 40 END,
       CASE WHEN i % 3 = 1 THEN 5000 + (i % 10) * 1000 END,
       CASE WHEN i % 3 = 1 THEN 'RUB' END,
       CASE WHEN i % 3 = 2 THEN 3 + i % 27 END,
       CASE WHEN i % 4 = 0 THEN 1000 END, 1,
       :'t0'::timestamptz - make_interval(days => 700 - (i * 700 / :npromo)),
       CASE WHEN i % 5 = 0 THEN :'t0'::timestamptz - make_interval(days => 600 - (i * 700 / :npromo)) END,
       i % 7 = 0, i % 9 <> 0, NULL,
       :'t0'::timestamptz - make_interval(days => 700 - (i * 700 / :npromo)),
       :'t0'::timestamptz - make_interval(days => 700 - (i * 700 / :npromo))
FROM generate_series(1, :npromo) i;

INSERT INTO promo_redemptions (promo_id, client_id, order_id, subscription_id, status, created_at, used_at)
SELECT 1 + floor(perf_gen.r(o.oseed, 92) * :npromo)::bigint, o.client_id, o.id, o.subscription_id, 'used', o.created_at, o.paid_at
FROM perf_gen.ord o WHERE o.cls = 'sub' AND o.status = 'fulfilled' AND perf_gen.r(o.oseed, 93) < 0.09 ORDER BY o.id;

INSERT INTO promo_links (id, code, name, squads, days, devices, max_activations, activations, enabled, created_at, updated_at)
OVERRIDING SYSTEM VALUE
SELECT i, 'P' || lpad(i::text, 4, '0'), 'Ссылка для блогера ' || i, ARRAY[perf_gen.uuid(1, 1)], 7, 1, NULL, 0, i % 6 <> 0,
       :'t0'::timestamptz - make_interval(days => 720 - i * 15), :'t0'::timestamptz - make_interval(days => 720 - i * 15)
FROM generate_series(1, 40) i;

INSERT INTO promo_link_activations (link_id, client_id, order_id, activated_at)
SELECT 1 + c.seed % 40, o.client_id, o.id, o.created_at
FROM perf_gen.ord o JOIN perf_gen.c c ON c.id = o.client_id WHERE o.cls = 'grant' ORDER BY o.id;
UPDATE promo_links l SET activations = a.n FROM (SELECT link_id, count(*) AS n FROM promo_link_activations GROUP BY link_id) a WHERE a.link_id = l.id;

INSERT INTO gifts (code, order_id, giver_id, tariff_id, period_id, days, tariff_name, status, expires_at, redeemed_by, redeemed_at, grant_order_id, canceled_at, created_at, updated_at)
SELECT upper(substr(md5(o.oseed::text), 1, 4)) || '-' || upper(substr(md5(o.oseed::text), 5, 4)) || '-' || upper(substr(md5(o.oseed::text), 9, 4)),
       o.id, o.client_id, 2, 5, 30, 'Стандарт',
       CASE WHEN perf_gen.r(o.oseed, 94) < 0.05 THEN 'canceled' ELSE 'active' END,
       o.paid_at + interval '180 days', NULL, NULL, NULL,
       CASE WHEN perf_gen.r(o.oseed, 94) < 0.05 THEN o.paid_at + interval '3 days' END,
       o.paid_at, o.paid_at
FROM perf_gen.ord o WHERE o.cls = 'gift_buy' ORDER BY o.id;

CREATE UNLOGGED TABLE perf_gen.tk0 AS
SELECT t AS tseed,
       c.id AS client_id, c.created_at AS client_created,
       c.created_at + (:'t0'::timestamptz - c.created_at) * perf_gen.r(t, 101) AS created_at,
       2 + floor(perf_gen.r(t, 102) * 6)::int AS msgs,
       perf_gen.r(t, 103) < 0.05 AS open
FROM generate_series(1::bigint, :ntickets) t
JOIN perf_gen.c c ON c.id = 1 + floor(power(perf_gen.r(t, 100), 0.7) * (:nc - 1))::bigint;

CREATE UNLOGGED TABLE perf_gen.tk AS
SELECT row_number() OVER (ORDER BY created_at, tseed) AS id, t.* FROM perf_gen.tk0 t;
DROP TABLE perf_gen.tk0;

CREATE UNLOGGED TABLE perf_gen.tm0 AS
SELECT t.id AS ticket_id, t.tseed, m,
       CASE WHEN m % 2 = 1 THEN 'client' ELSE 'support' END AS author,
       CASE WHEN t.open AND m = t.msgs THEN 'client' WHEN m % 2 = 1 THEN 'client' ELSE 'support' END AS author2,
       t.created_at + make_interval(secs => (m - 1) * 1800 + perf_gen.r(t.tseed * 16 + m, 104) * 600) AS created_at
FROM perf_gen.tk t CROSS JOIN LATERAL generate_series(1, t.msgs) m;
CREATE UNLOGGED TABLE perf_gen.tm AS
SELECT row_number() OVER (ORDER BY created_at, tseed, m) AS id, x.* FROM perf_gen.tm0 x;
DROP TABLE perf_gen.tm0;
CREATE INDEX ON perf_gen.tm (ticket_id, m);
ANALYZE perf_gen.tm;

CREATE UNLOGGED TABLE perf_gen.tickets AS
SELECT t.id, t.client_id, NULL::bigint AS subscription_id,
       perf_gen.pick(ARRAY['bot', 'bot', 'bot', 'cabinet', 'miniapp'], perf_gen.r(t.tseed, 105)) AS channel,
       CASE WHEN t.open THEN 'open' ELSE 'closed' END AS status,
       perf_gen.pick(ARRAY['Не подключается', 'Вопрос по оплате', 'Медленно работает', 'Как продлить', 'Не приходит ссылка'], perf_gen.r(t.tseed, 106)) AS subject,
       lm.author2 AS last_author,
       t.created_at,
       lm.created_at AS updated_at,
       CASE WHEN NOT t.open THEN lm.created_at + interval '1 day' END AS closed_at,
       lm.id AS last_message_id,
       CASE WHEN lm.author2 = 'client' THEN lm.id END AS waiting_message_id
FROM perf_gen.tk t
JOIN LATERAL (SELECT id, author2, created_at FROM perf_gen.tm WHERE ticket_id = t.id ORDER BY m DESC LIMIT 1) lm ON true;
CALL perf_gen.load('tickets', 'id, client_id, subscription_id, channel, status, subject, last_author, created_at, updated_at, closed_at, last_message_id, waiting_message_id', 'perf_gen.tickets');
DROP TABLE perf_gen.tickets;

CREATE UNLOGGED TABLE perf_gen.ticket_messages AS
SELECT m.id, m.ticket_id, m.author2 AS author,
       CASE WHEN m.author2 = 'client'
            THEN perf_gen.pick(ARRAY['Здравствуйте, не могу подключиться с телефона, пишет ошибка соединения.',
                                     'Оплатил, но подписка не продлилась. Проверьте, пожалуйста.',
                                     'Скорость очень низкая вечером, что можно сделать?',
                                     'Как добавить ещё одно устройство?',
                                     'Спасибо, всё заработало!'], perf_gen.r(m.tseed * 16 + m.m, 107))
            ELSE perf_gen.pick(ARRAY['Здравствуйте! Обновите подписку в приложении и попробуйте снова.',
                                     'Проверили: оплата прошла, срок продлён.',
                                     'Попробуйте другой сервер из списка, вечером там свободнее.',
                                     'Устройство можно добавить в разделе «Подписка».'], perf_gen.r(m.tseed * 16 + m.m, 108)) END AS body,
       m.created_at,
       m.created_at + interval '5 minutes' AS read_at
FROM perf_gen.tm m;
CALL perf_gen.load('ticket_messages', 'id, ticket_id, author, body, created_at, read_at', 'perf_gen.ticket_messages');
DROP TABLE perf_gen.ticket_messages;

CREATE UNLOGGED TABLE perf_gen.ev0 AS
SELECT 'client_registered' AS kind, c.id AS client_id, c.created_at AS at,
       jsonb_build_object('type', 'client_registered', 'source', CASE WHEN c.imported THEN 'migration' WHEN c.tg THEN 'bot' ELSE 'web' END,
                          'client_id', c.id, 'start_param', NULL) AS payload
FROM perf_gen.c c
UNION ALL
SELECT 'payment_paid', p.client_id, p.paid_at,
       jsonb_build_object('type', 'payment_paid', 'amount', jsonb_build_object('minor', p.amount_minor, 'currency', p.currency),
                          'order_id', p.order_id, 'provider', p.provider, 'client_id', p.client_id, 'order_kind', o.kind, 'payment_id', p.id)
FROM perf_gen.paid p JOIN perf_gen.ord o ON o.id = p.order_id
UNION ALL
SELECT 'order_fulfilled', o.client_id, o.paid_at + interval '5 seconds',
       jsonb_build_object('type', 'order_fulfilled', 'order_id', o.id, 'client_id', o.client_id, 'order_kind', o.kind, 'subscription_id', o.subscription_id)
FROM perf_gen.ord o WHERE o.status IN ('fulfilled', 'refunded')
UNION ALL
SELECT 'subscription_renewed', o.client_id, o.paid_at + interval '6 seconds',
       jsonb_build_object('type', 'subscription_renewed', 'client_id', o.client_id, 'subscription_id', o.subscription_id,
                          'expire_at', to_char((o.paid_at + make_interval(days => (ARRAY[30, 90, 180, 365])[o.pidx])) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'))
FROM perf_gen.ord o WHERE o.kind = 'renewal' AND o.status = 'fulfilled' AND o.subscription_id IS NOT NULL
UNION ALL
SELECT 'subscription_created', s.client_id, s.created_at,
       jsonb_build_object('type', 'subscription_created', 'client_id', s.client_id, 'subscription_id', s.id, 'kind', s.kind)
FROM perf_gen.sub s
UNION ALL
SELECT 'subscription_status_changed', s.client_id, s.expire_at,
       jsonb_build_object('type', 'subscription_status_changed', 'client_id', s.client_id, 'subscription_id', s.id, 'from', 'active', 'to', 'expired')
FROM perf_gen.sub s WHERE s.expire_at < :'t0'::timestamptz
UNION ALL
SELECT 'subscription_status_changed', s.client_id, s.created_at + interval '1 day',
       jsonb_build_object('type', 'subscription_status_changed', 'client_id', s.client_id, 'subscription_id', s.id, 'from', 'active', 'to', 'limited')
FROM perf_gen.sub s WHERE perf_gen.r(s.seed, 110) < 0.12 AND s.created_at + interval '1 day' < :'t0'::timestamptz
UNION ALL
SELECT 'subscription_status_changed', s.client_id, s.created_at + interval '2 days',
       jsonb_build_object('type', 'subscription_status_changed', 'client_id', s.client_id, 'subscription_id', s.id, 'from', 'limited', 'to', 'active')
FROM perf_gen.sub s WHERE perf_gen.r(s.seed, 110) < 0.12 AND s.created_at + interval '2 days' < :'t0'::timestamptz
UNION ALL
SELECT 'balance_changed', l.client_id, l.created_at,
       jsonb_build_object('type', 'balance_changed', 'client_id', l.client_id, 'wallet', 'main', 'kind', CASE WHEN l.kind = 'topup' THEN 'top_up' ELSE 'purchase' END,
                          'delta', perf_gen.money(l.delta))
FROM perf_gen.ledger l
UNION ALL
SELECT 'ticket_opened', t.client_id, t.created_at, jsonb_build_object('type', 'ticket_opened', 'client_id', t.client_id, 'ticket_id', t.id)
FROM perf_gen.tk t
UNION ALL
SELECT 'ticket_answered', t.client_id, m.created_at, jsonb_build_object('type', 'ticket_answered', 'client_id', t.client_id, 'ticket_id', t.id)
FROM perf_gen.tm m JOIN perf_gen.tk t ON t.id = m.ticket_id WHERE m.author2 = 'support'
UNION ALL
SELECT 'client_blocked', c.id, c.created_at + interval '20 days', jsonb_build_object('type', 'client_blocked', 'client_id', c.id, 'reason', 'нарушение правил')
FROM perf_gen.c c WHERE c.blocked AND c.created_at + interval '20 days' < :'t0'::timestamptz;

CREATE UNLOGGED TABLE perf_gen.domain_events AS
SELECT row_number() OVER (ORDER BY at, client_id, kind) AS id, kind, client_id, payload, at AS occurred_at, at + interval '1 second' AS dispatched_at
FROM perf_gen.ev0 WHERE at < :'t0'::timestamptz;
DROP TABLE perf_gen.ev0;
CALL perf_gen.load('domain_events', 'id, kind, client_id, payload, occurred_at, dispatched_at', 'perf_gen.domain_events', 100000);

INSERT INTO event_deliveries (event_id, module, status, attempts, created_at, updated_at)
SELECT e.id, 'referral', 'done', 1, e.occurred_at, e.dispatched_at
FROM perf_gen.domain_events e WHERE e.kind = 'payment_paid' AND EXISTS (SELECT 1 FROM perf_gen.ref r WHERE r.client_id = e.client_id)
ORDER BY e.id;
INSERT INTO event_deliveries (event_id, module, status, attempts, last_error, created_at, updated_at)
SELECT e.id, 'contests', 'review', 10, 'обработчик не ответил', e.occurred_at, e.dispatched_at
FROM perf_gen.domain_events e WHERE e.kind = 'payment_paid' AND e.id % 50000 = 7 ORDER BY e.id;
DROP TABLE perf_gen.domain_events;

CREATE UNLOGGED TABLE perf_gen.au0 AS
SELECT 'system' AS actor_kind, NULL::bigint AS actor_id, 'order.fulfilled' AS action, 'order' AS entity_kind, o.id AS entity_id,
       jsonb_build_object('order_kind', o.kind, 'subscription_id', o.subscription_id) AS details, o.paid_at + interval '5 seconds' AS at
FROM perf_gen.ord o WHERE o.status IN ('fulfilled', 'refunded')
UNION ALL
SELECT 'system', NULL, 'autorenew.reminder', 'subscription', o.subscription_id, jsonb_build_object('client_id', o.client_id, 'days_left', 1),
       o.paid_at - interval '1 day'
FROM perf_gen.ord o JOIN perf_gen.c c ON c.id = o.client_id
WHERE o.cls = 'sub' AND o.kind = 'renewal' AND c.autorenew AND o.subscription_id IS NOT NULL
UNION ALL
SELECT 'system', NULL, 'subscription.ended', 'subscription', s.id, jsonb_build_object('client_id', s.client_id, 'reason', 'expired'), s.expire_at + interval '3 days'
FROM perf_gen.sub s WHERE s.expire_at + interval '3 days' < :'t0'::timestamptz AND s.kind <> 'trial'
UNION ALL
SELECT 'system', NULL, 'autorenew.reminder', 'subscription', s.id, jsonb_build_object('client_id', s.client_id, 'days_left', d), s.expire_at - make_interval(days => d)
FROM perf_gen.sub s CROSS JOIN unnest(ARRAY[3, 1]) d WHERE s.autorenew AND s.expire_at - make_interval(days => d) < :'t0'::timestamptz
UNION ALL
SELECT 'admin', 1, 'subscription.add_days', 'subscription', s.id,
       jsonb_build_object('at', s.created_at + interval '5 days', 'days', 7, 'status', 'active', 'client_id', s.client_id),
       s.created_at + interval '5 days'
FROM perf_gen.sub s WHERE perf_gen.r(s.seed, 120) < 0.06 AND s.created_at + interval '5 days' < :'t0'::timestamptz
UNION ALL
SELECT 'admin', 1, 'client.note', 'client', c.id, jsonb_build_object('note', 'Постоянный клиент'), c.created_at + interval '10 days'
FROM perf_gen.c c WHERE perf_gen.r(c.seed, 121) < 0.01 AND c.created_at + interval '10 days' < :'t0'::timestamptz
UNION ALL
SELECT 'admin', 1, 'client.block', 'client', c.id, jsonb_build_object('reason', 'нарушение правил', 'changed', true), c.created_at + interval '20 days'
FROM perf_gen.c c WHERE c.blocked AND c.created_at + interval '20 days' < :'t0'::timestamptz
UNION ALL
SELECT 'client', c.id, 'client.totp_enabled', 'client', c.id, '{}'::jsonb, c.created_at + interval '2 days'
FROM perf_gen.c c WHERE perf_gen.r(c.seed, 122) < 0.01 AND c.created_at + interval '2 days' < :'t0'::timestamptz
UNION ALL
SELECT 'system', NULL, 'payment.amount_mismatch', 'payment', p.id, jsonb_build_object('expected', p.amount_minor), p.created_at + interval '1 minute'
FROM perf_gen.paid p WHERE p.id % 997 = 1;

CREATE UNLOGGED TABLE perf_gen.audit_log AS
SELECT row_number() OVER (ORDER BY at, entity_kind, entity_id, action) AS id, actor_kind, actor_id, action, entity_kind, entity_id, details,
       CASE WHEN actor_kind = 'admin' THEN '203.0.113.10' END AS ip, at AS created_at
FROM perf_gen.au0 WHERE at < :'t0'::timestamptz;
DROP TABLE perf_gen.au0;
CALL perf_gen.load('audit_log', 'id, actor_kind, actor_id, action, entity_kind, entity_id, details, ip, created_at', 'perf_gen.audit_log', 100000);
DROP TABLE perf_gen.audit_log;

CREATE UNLOGGED TABLE perf_gen.re0 AS
SELECT s.id AS sub_id, s.seed, k,
       s.created_at + (least(:'t0'::timestamptz, s.expire_at + interval '3 days') - s.created_at) * ((k + perf_gen.r(s.seed * 32 + k, 130) * 0.9) / 12.0) AS at,
       CASE WHEN k = 0 THEN 'user.created' WHEN k = 1 THEN 'user.first_connected' WHEN k = 11 AND s.expire_at < :'t0'::timestamptz THEN 'user.expired'
            WHEN k = 10 THEN 'user.expires_in_72_hours' WHEN k % 3 = 0 THEN 'user.bandwidth_usage_threshold_reached' ELSE 'user.modified' END AS event
FROM perf_gen.sub s CROSS JOIN generate_series(0, 11) k
WHERE s.role = 'main' AND (k < 6 OR perf_gen.r(s.seed, 131) < 0.8);

CREATE UNLOGGED TABLE perf_gen.remna_events AS
SELECT row_number() OVER (ORDER BY at, sub_id, k) AS id, sha256(convert_to(sub_id::text || ':' || k::text, 'UTF8')) AS body_sha256,
       'user' AS scope, event, at AS occurred_at, 'user' AS subject_kind, sub_id::text AS subject_id,
       jsonb_build_object('event', event, 'scope', 'user', 'timestamp', to_char(at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
                          'data', jsonb_build_object('id', sub_id, 'uuid', perf_gen.uuid(sub_id, 500), 'username', 'u' || sub_id,
                                                     'status', CASE WHEN event = 'user.expired' THEN 'EXPIRED' ELSE 'ACTIVE' END,
                                                     'shortUuid', substr(md5(sub_id::text || 'short'), 1, 16),
                                                     'usedTrafficBytes', (perf_gen.r(seed * 32 + k, 132) * 50000000000)::bigint,
                                                     'trafficLimitBytes', 0, 'trafficLimitStrategy', 'NO_RESET', 'hwidDeviceLimit', 3,
                                                     'expireAt', to_char((at + interval '20 days') AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'))) AS payload,
       at + interval '1 second' AS received_at, at + interval '2 seconds' AS processed_at
FROM perf_gen.re0 WHERE at < :'t0'::timestamptz;
DROP TABLE perf_gen.re0;
CALL perf_gen.load('remna_events', 'id, body_sha256, scope, event, occurred_at, subject_kind, subject_id, payload, received_at, processed_at', 'perf_gen.remna_events', 100000);
DROP TABLE perf_gen.remna_events;

INSERT INTO remna_events (body_sha256, scope, event, occurred_at, subject_kind, subject_id, payload, received_at, processed_at)
SELECT sha256(convert_to('node:' || i, 'UTF8')), 'node',
       CASE WHEN i % 2 = 0 THEN 'node.connection_lost' ELSE 'node.connection_restored' END,
       :'t0'::timestamptz - make_interval(secs => (20000 - i) * 3000), 'node', perf_gen.uuid(1 + i % 12, 300)::text,
       jsonb_build_object('event', CASE WHEN i % 2 = 0 THEN 'node.connection_lost' ELSE 'node.connection_restored' END, 'scope', 'node',
                          'data', jsonb_build_object('name', 'NODE-' || (1 + i % 12), 'uuid', perf_gen.uuid(1 + i % 12, 300), 'address', '198.51.100.' || (1 + i % 12),
                                                     'isConnected', i % 2 = 1, 'isDisabled', false)),
       :'t0'::timestamptz - make_interval(secs => (20000 - i) * 3000), :'t0'::timestamptz - make_interval(secs => (20000 - i) * 3000)
FROM generate_series(1, 20000) i;

CREATE UNLOGGED TABLE perf_gen.wh0 AS
SELECT p.id AS payment_id, p.provider, p.status, p.created_at, p.paid_at, p.amount_minor, p.provider_txid, p.public_id, d,
       CASE WHEN d = 1 AND p.status = 'paid' THEN 'accepted' WHEN d = 1 AND p.status IN ('failed', 'canceled') THEN 'ignored'
            WHEN d = 1 THEN 'accepted' ELSE 'duplicate' END AS outcome,
       COALESCE(p.paid_at, p.created_at + interval '20 minutes') + make_interval(secs => d * 40) AS at
FROM perf_gen.payrecent p
CROSS JOIN LATERAL generate_series(1, CASE WHEN perf_gen.r(p.id, 140) < 0.3 THEN 2 ELSE 1 END) d
WHERE p.status IN ('paid', 'failed', 'canceled', 'refunded', 'partially_refunded')
UNION ALL
SELECT NULL, perf_gen.pick(ARRAY['yookassa', 'platega', 'cryptopay', 'heleket'], perf_gen.r(i, 141)), NULL, NULL, NULL, NULL, NULL, NULL, 1,
       CASE WHEN i % 4 = 0 THEN 'rejected_signature' WHEN i % 4 = 1 THEN 'payment_not_found' WHEN i % 4 = 2 THEN 'error' ELSE 'rejected_business' END,
       :'t0'::timestamptz - make_interval(secs => perf_gen.r(i, 142) * 86400 * CASE WHEN i % 4 = 0 THEN 3 ELSE 29 END)
FROM generate_series(1, greatest(1, round(4000 * :scale))::int) i;

CREATE UNLOGGED TABLE perf_gen.webhook_inbox AS
SELECT row_number() OVER (ORDER BY at, payment_id, d) AS id, w.*
FROM perf_gen.wh0 w WHERE w.at < :'t0'::timestamptz;
DROP TABLE perf_gen.wh0;

CREATE UNLOGGED TABLE perf_gen.webhooks AS
SELECT w.id, w.provider AS source,
       w.provider || ':' || COALESCE(w.provider_txid, 'x') || ':' || w.id AS dedupe_key,
       '185.71.76.' || (w.id % 250) AS remote_ip,
       jsonb_build_object('content-type', 'application/json', 'user-agent', w.provider || '-notifier/1.0') AS headers,
       CASE WHEN w.outcome = 'rejected_signature' THEN NULL
            ELSE convert_to(jsonb_build_object('type', 'notification', 'event', CASE WHEN w.status = 'paid' THEN 'payment.succeeded' ELSE 'payment.canceled' END,
                                               'object', jsonb_build_object('id', COALESCE(w.provider_txid, 'unknown-' || w.id), 'status', COALESCE(w.status, 'succeeded'),
                                                                            'amount', jsonb_build_object('value', round(COALESCE(w.amount_minor, 19900) / 100.0, 2)::text, 'currency', 'RUB'),
                                                                            'metadata', jsonb_build_object('payment', COALESCE(w.public_id::text, perf_gen.uuid(w.id, 600)::text)),
                                                                            'payment_method', jsonb_build_object('type', 'bank_card', 'title', 'Bank card *4444'))) ::text, 'UTF8') END AS body,
       w.outcome,
       w.payment_id,
       CASE WHEN w.outcome = 'error' THEN 'не удалось обработать уведомление' WHEN w.outcome = 'payment_not_found' THEN 'платёж не найден' END AS error,
       w.at AS received_at,
       w.at + interval '200 milliseconds' AS processed_at,
       CASE WHEN w.outcome <> 'rejected_signature' THEN jsonb_build_object('provider_txid', COALESCE(w.provider_txid, 'unknown-' || w.id),
                                                                          'status', COALESCE(w.status, 'paid'),
                                                                          'amount', jsonb_build_object('minor', COALESCE(w.amount_minor, 19900), 'currency', 'RUB')) END AS parsed,
       CASE WHEN w.status = 'paid' THEN 'payment.succeeded' WHEN w.status IS NULL THEN 'payment.succeeded' ELSE 'payment.canceled' END AS event,
       jsonb_build_array(jsonb_build_object('at', w.at, 'step', 'delivery'),
                         jsonb_build_object('at', w.at + interval '200 milliseconds', 'step', 'processed', 'outcome', w.outcome)) AS trail,
       w.d AS deliveries
FROM perf_gen.webhook_inbox w;
CALL perf_gen.load('webhook_inbox', 'id, source, dedupe_key, remote_ip, headers, body, outcome, payment_id, error, received_at, processed_at, parsed, event, trail, deliveries', 'perf_gen.webhooks', 50000);
DROP TABLE perf_gen.webhooks;
DROP TABLE perf_gen.webhook_inbox;
DROP TABLE perf_gen.payrecent;

CREATE UNLOGGED TABLE perf_gen.jb0 AS
SELECT 'fulfill_order' AS kind, 'fulfill:' || o.id AS dedupe_key, jsonb_build_object('kind', 'fulfill_order', 'order_id', o.id) AS payload,
       o.paid_at AS at
FROM perf_gen.ord o WHERE o.status IN ('fulfilled', 'refunded')
UNION ALL
SELECT 'notify', NULL, jsonb_build_object('kind', 'notify', 'client_id', s.client_id, 'template', 'subscription_expiring', 'params', jsonb_build_object('days', 3)),
       s.expire_at - interval '3 days'
FROM perf_gen.sub s WHERE s.expire_at - interval '3 days' < :'t0'::timestamptz
UNION ALL
SELECT 'notify', NULL, jsonb_build_object('kind', 'notify', 'client_id', s.client_id, 'template', 'subscription_expired', 'params', '{}'::jsonb),
       s.expire_at
FROM perf_gen.sub s WHERE s.expire_at < :'t0'::timestamptz;

CREATE UNLOGGED TABLE perf_gen.jobs AS
SELECT row_number() OVER (ORDER BY at, kind, dedupe_key) AS id, kind, dedupe_key, payload,
       'done' AS status,
       at AS run_at, 1 AS attempts, at AS created_at, at + interval '2 seconds' AS updated_at,
       at + interval '2 seconds' AS done_at
FROM perf_gen.jb0;
DROP TABLE perf_gen.jb0;
CALL perf_gen.load('jobs', 'id, kind, dedupe_key, payload, status, run_at, attempts, created_at, updated_at, done_at', 'perf_gen.jobs', 100000);
DROP TABLE perf_gen.jobs;

INSERT INTO notifications_sent (client_id, kind, cycle_key, sent_at)
SELECT DISTINCT ON (s.client_id, k.kind, to_char(s.expire_at, 'YYYYMMDD'))
       s.client_id, k.kind, to_char(s.expire_at, 'YYYYMMDD'), s.expire_at - make_interval(days => k.d)
FROM perf_gen.sub s CROSS JOIN (VALUES ('expiring_3d', 3), ('expiring_1d', 1)) k(kind, d)
WHERE s.expire_at - make_interval(days => k.d) < :'t0'::timestamptz;

INSERT INTO autorenew_attempts (subscription_id, period_key, attempt_no, result, details, created_at)
SELECT s.id, to_char(s.expire_at, 'YYYY-MM-DD'), 1,
       CASE WHEN s.expire_at > :'t0'::timestamptz THEN 'reminded' WHEN perf_gen.r(s.seed, 150) < 0.6 THEN 'insufficient' ELSE 'failed' END,
       NULL, s.expire_at - interval '1 day'
FROM perf_gen.sub s WHERE s.autorenew AND s.expire_at - interval '1 day' < :'t0'::timestamptz ORDER BY s.id;

INSERT INTO client_sessions (client_id, token_hash, channel, scope, ip, user_agent, created_at, last_seen_at, expires_at, revoked_at)
SELECT c.id, sha256(convert_to('sess:' || c.seed || ':' || k, 'UTF8')), CASE WHEN k % 2 = 0 THEN 'miniapp' ELSE 'cabinet' END, 'full',
       '198.51.100.' || (c.seed % 250), 'Mozilla/5.0', x.at, x.at + interval '2 hours', x.at + interval '30 days',
       CASE WHEN x.at < :'t0'::timestamptz - interval '30 days' THEN x.at + interval '30 days' END
FROM perf_gen.c c
CROSS JOIN LATERAL generate_series(1, CASE WHEN perf_gen.r(c.seed, 151) < 0.35 THEN 1 + floor(perf_gen.r(c.seed, 152) * 3)::int ELSE 0 END) k
CROSS JOIN LATERAL (SELECT c.created_at + (:'t0'::timestamptz - c.created_at) * perf_gen.r(c.seed * 8 + k, 153) AS at) x
ORDER BY x.at;

INSERT INTO bot_consents (telegram_id, version, documents, accepted_at)
SELECT 5000000000 + c.seed * 13, 'v1', '[]'::jsonb, c.created_at FROM perf_gen.c c WHERE c.tg ORDER BY c.id;

INSERT INTO saved_payment_methods (client_id, provider, token_nonce, token_ciphertext, title, created_at)
SELECT c.id, 'yookassa', '\x000102030405060708090a0b'::bytea, sha256(convert_to('card:' || c.seed, 'UTF8')), 'Карта *' || lpad((c.seed % 10000)::text, 4, '0'), c.start_at
FROM perf_gen.c c WHERE c.paying AND c.autorenew AND perf_gen.r(c.seed, 154) < 0.4 ORDER BY c.id;

INSERT INTO broadcast_images (id, content_type, bytes, width, height, created_at) OVERRIDING SYSTEM VALUE
VALUES (1, 'image/png', '\x89504e470d0a1a0a'::bytea, 1, 1, :'t0'::timestamptz - interval '200 days');

INSERT INTO broadcasts (id, text, image_id, buttons, audience, status, send_at, started_at, finished_at, total, created_by, created_at, updated_at, channel)
OVERRIDING SYSTEM VALUE
SELECT i, 'Новости сервиса №' || i || ': добавили новые сервера и ускорили подключение.', CASE WHEN i % 3 = 0 THEN 1 END, '[]'::jsonb,
       jsonb_build_object('kind', CASE WHEN i = 40 THEN 'all' ELSE (ARRAY['active', 'expired', 'trial', 'all'])[1 + i % 4] END),
       'done', NULL, :'t0'::timestamptz - make_interval(days => 400 - i * 10), :'t0'::timestamptz - make_interval(days => 400 - i * 10) + interval '2 hours',
       0, NULL, :'t0'::timestamptz - make_interval(days => 400 - i * 10) - interval '1 hour', :'t0'::timestamptz - make_interval(days => 400 - i * 10) + interval '2 hours', 'bot'
FROM generate_series(1, 40) i;

CREATE UNLOGGED TABLE perf_gen.br AS
SELECT row_number() OVER (ORDER BY b, c.id) AS id, b AS broadcast_id, c.id AS client_id, 5000000000 + c.seed * 13 AS chat_id,
       CASE WHEN c.bot_unreachable THEN 'blocked' WHEN perf_gen.r(c.seed * 64 + b, 160) < 0.01 THEN 'failed' ELSE 'sent' END AS state,
       1 AS attempts,
       :'t0'::timestamptz - make_interval(days => 400 - b * 10) + make_interval(secs => perf_gen.r(c.seed * 64 + b, 161) * 7000) AS done_at
FROM (SELECT c.*, perf_gen.r(c.seed, 17) < 0.06 AS bot_unreachable FROM perf_gen.c c) c
CROSS JOIN (VALUES (37), (40)) bb(b)
WHERE c.tg AND NOT c.deleted AND c.created_at < :'t0'::timestamptz - make_interval(days => 400 - b * 10)
  AND (b = 40 OR perf_gen.r(c.seed, 162) < 0.4);
INSERT INTO broadcast_recipients (broadcast_id, client_id, chat_id, state, attempts, done_at, channel)
SELECT broadcast_id, client_id, chat_id, state, attempts, done_at, 'bot' FROM perf_gen.br ORDER BY id;
UPDATE broadcasts b SET total = x.n FROM (SELECT broadcast_id, count(*) AS n FROM perf_gen.br GROUP BY broadcast_id) x WHERE x.broadcast_id = b.id;
DROP TABLE perf_gen.br;

INSERT INTO auto_broadcasts (id, trigger, days, with_autorenew, channel, text, enabled, created_at, updated_at) OVERRIDING SYSTEM VALUE VALUES
    (1, 'expiring', 3, false, 'bot', 'Подписка закончится через 3 дня. Продлите, чтобы не потерять доступ.', true, :'t0'::timestamptz - interval '300 days', :'t0'::timestamptz - interval '300 days'),
    (2, 'expired', 1, false, 'bot', 'Подписка закончилась. Вернитесь со скидкой!', true, :'t0'::timestamptz - interval '300 days', :'t0'::timestamptz - interval '300 days'),
    (3, 'trial_ended', 0, false, 'bot', 'Пробный период закончился. Выберите тариф.', true, :'t0'::timestamptz - interval '300 days', :'t0'::timestamptz - interval '300 days'),
    (4, 'no_purchase', 7, false, 'bot', 'Вы ещё не попробовали платный тариф.', true, :'t0'::timestamptz - interval '300 days', :'t0'::timestamptz - interval '300 days'),
    (5, 'not_connected', 2, false, 'bot', 'Нужна помощь с подключением?', false, :'t0'::timestamptz - interval '300 days', :'t0'::timestamptz - interval '300 days');

INSERT INTO auto_broadcast_cases (rule_id, case_key, client_id, due_at, broadcast_id, queued, created_at)
SELECT CASE WHEN s.kind = 'trial' THEN 3 ELSE 2 END, s.id || ':' || to_char(s.expire_at, 'YYYYMMDD'), s.client_id, s.expire_at + interval '1 day', NULL, true, s.expire_at + interval '1 day'
FROM perf_gen.sub s WHERE s.expire_at + interval '1 day' < :'t0'::timestamptz AND s.expire_at > :'t0'::timestamptz - interval '300 days'
ORDER BY s.id;

INSERT INTO contests (id, title, description, prize, winners, conditions, starts_at, ends_at, seed, seed_hash, list_hash, entries_drawn, drawn_at, created_at, updated_at)
OVERRIDING SYSTEM VALUE
SELECT i, 'Розыгрыш №' || i, 'Подписка на месяц победителям', jsonb_build_object('kind', 'days', 'days', 30), 3, '{}'::jsonb,
       :'t0'::timestamptz - make_interval(days => 300 - i * 20), :'t0'::timestamptz - make_interval(days => 290 - i * 20),
       sha256(convert_to('contest' || i, 'UTF8')), encode(sha256(sha256(convert_to('contest' || i, 'UTF8'))), 'hex'),
       CASE WHEN i < 15 THEN md5('list' || i) END, CASE WHEN i < 15 THEN 1000 END, CASE WHEN i < 15 THEN :'t0'::timestamptz - make_interval(days => 290 - i * 20) END,
       :'t0'::timestamptz - make_interval(days => 301 - i * 20), :'t0'::timestamptz - make_interval(days => 290 - i * 20)
FROM generate_series(1, 15) i;

INSERT INTO contest_entries (contest_id, client_id, joined_at, place, prize_state)
SELECT x.contest_id, x.client_id, x.joined_at,
       CASE WHEN x.rn <= 3 AND x.contest_id < 15 THEN x.rn END,
       CASE WHEN x.rn <= 3 AND x.contest_id < 15 THEN 'issued' END
FROM (
    SELECT i AS contest_id, c.id AS client_id, :'t0'::timestamptz - make_interval(days => 300 - i * 20) + interval '1 hour' AS joined_at,
           row_number() OVER (PARTITION BY i ORDER BY perf_gen.r(c.seed * 16 + i, 170)) AS rn
    FROM generate_series(1, 15) i
    JOIN perf_gen.c c ON perf_gen.r(c.seed * 16 + i, 171) < CASE WHEN i = 14 THEN 0.5 ELSE 0.03 END
    WHERE c.created_at < :'t0'::timestamptz - make_interval(days => 300 - i * 20) AND NOT c.deleted
) x ORDER BY x.contest_id, x.client_id;

INSERT INTO roulette_config (id, config, version) VALUES (1, jsonb_build_object('enabled', true, 'sectors', '[]'::jsonb), 1);
INSERT INTO roulette_spins (client_id, request_id, source, ticket_id, config_version, sector, prize, label, balance_minor, days, promo, state, created_at, issued_at)
SELECT c.id, perf_gen.uuid(c.seed * 8 + k, 180), 'free', NULL, 1, k % 6, jsonb_build_object('kind', CASE WHEN k % 3 = 0 THEN 'days' ELSE 'empty' END),
       CASE WHEN k % 3 = 0 THEN '+3 дня' ELSE 'Пусто' END, 0, CASE WHEN k % 3 = 0 THEN 3 ELSE 0 END, 0,
       CASE WHEN k % 3 = 0 THEN 'issued' ELSE 'empty' END, x.at, CASE WHEN k % 3 = 0 THEN x.at END
FROM perf_gen.c c
CROSS JOIN LATERAL generate_series(1, CASE WHEN perf_gen.r(c.seed, 181) < 0.15 THEN 1 + floor(perf_gen.r(c.seed, 182) * 3)::int ELSE 0 END) k
CROSS JOIN LATERAL (SELECT greatest(c.created_at, :'t0'::timestamptz - interval '120 days') + (:'t0'::timestamptz - greatest(c.created_at, :'t0'::timestamptz - interval '120 days')) * perf_gen.r(c.seed * 8 + k, 183) AS at) x
ORDER BY x.at;

INSERT INTO antibot_marks (client_id, reasons, details, marked_at, confirmed_at, cleared_at)
SELECT c.id, '["same_device"]'::jsonb, '{}'::jsonb, c.created_at + interval '1 hour',
       CASE WHEN perf_gen.r(c.seed, 190) < 0.3 THEN c.created_at + interval '2 hours' END,
       CASE WHEN perf_gen.r(c.seed, 190) >= 0.7 THEN c.created_at + interval '1 day' END
FROM perf_gen.c c WHERE perf_gen.r(c.seed, 191) < 0.03 ORDER BY c.id;
INSERT INTO antibot_events (client_id, action, reasons, details, created_at)
SELECT m.client_id, a.action, '["same_device"]'::jsonb, '{}'::jsonb, m.marked_at + a.shift
FROM antibot_marks m CROSS JOIN (VALUES ('marked', interval '0'), ('trial_denied', interval '1 minute'), ('asked', interval '2 minutes')) a(action, shift)
ORDER BY m.marked_at;

INSERT INTO traffic_limit_rules (id, enabled, audience, period, limit_bytes, restriction, note) OVERRIDING SYSTEM VALUE VALUES
    (1, true, 'trial', 'day', 5368709120, 'none', 'Пробный период'),
    (2, true, 'all', 'week', 214748364800, 'none', 'Неделя');
INSERT INTO traffic_exceeds (rule_id, subscription_id, client_id, period, window_start, window_end, limit_bytes, used_bytes, warn, alert, restriction, exceeded_at, warned_at)
SELECT 2, s.id, s.client_id, 'week', (s.created_at + make_interval(days => k * 7))::date, (s.created_at + make_interval(days => k * 7 + 7)),
       214748364800, 220000000000, true, true, 'none', s.created_at + make_interval(days => k * 7 + 5), s.created_at + make_interval(days => k * 7 + 5)
FROM perf_gen.sub s CROSS JOIN generate_series(0, 2) k
WHERE s.role = 'main' AND perf_gen.r(s.seed, 195) < 0.02 AND s.created_at + make_interval(days => k * 7 + 5) < :'t0'::timestamptz
ORDER BY s.id, k;

INSERT INTO payment_notices (provider, event_key, kind, payment_id, client_id, amount_minor, currency, text, created_at)
SELECT p.provider, 'notice-' || p.id, CASE WHEN p.id % 2 = 0 THEN 'dispute' ELSE 'refund_reported' END, p.id, p.client_id, p.amount_minor, p.currency,
       'Платёжка сообщила о споре по платежу', p.paid_at + interval '5 days'
FROM perf_gen.paid p WHERE p.id % 5003 = 11 AND p.provider NOT IN ('balance') ORDER BY p.id;

DO $$
DECLARE
    t record;
BEGIN
    FOR t IN
        SELECT c.relname AS tbl, a.attname AS col
        FROM pg_attribute a
        JOIN pg_class c ON c.oid = a.attrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public' AND a.attidentity IN ('a', 'd') AND c.relkind = 'r'
    LOOP
        EXECUTE format('SELECT setval(pg_get_serial_sequence(%L, %L), GREATEST(1, COALESCE((SELECT max(%I) FROM %I), 0) + 1), false)',
                       t.tbl, t.col, t.col, t.tbl);
    END LOOP;
END $$;

DROP SCHEMA perf_gen CASCADE;
