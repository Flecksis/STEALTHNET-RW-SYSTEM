--
-- PostgreSQL database dump
--

\restrict rz42Vh73BFn6oBeD9oUj5dF6X0FXJGfiSqhBOzgBENQKHMBZDkukkCg4Qf2Hlvl

-- Dumped from database version 16.15 (Homebrew)
-- Dumped by pg_dump version 16.15 (Homebrew)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: autopay; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.autopay (
    telegram_id bigint NOT NULL,
    method text DEFAULT 'yookassa'::text NOT NULL,
    method_id text DEFAULT ''::text NOT NULL,
    title text DEFAULT ''::text NOT NULL,
    months integer DEFAULT 1 NOT NULL,
    amount text DEFAULT ''::text NOT NULL,
    currency text DEFAULT 'RUB'::text NOT NULL,
    enabled integer DEFAULT 1 NOT NULL,
    created_at text DEFAULT ''::text NOT NULL,
    last_pay_at text DEFAULT ''::text NOT NULL,
    next_try_at text DEFAULT ''::text NOT NULL,
    fails integer DEFAULT 0 NOT NULL,
    last_error text DEFAULT ''::text NOT NULL,
    paid_period text DEFAULT ''::text NOT NULL,
    plan_snapshot text DEFAULT ''::text NOT NULL
);


--
-- Name: backup_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.backup_state (
    id integer NOT NULL,
    data text DEFAULT ''::text NOT NULL
);


--
-- Name: email_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.email_tokens (
    hash text NOT NULL,
    tg_id bigint NOT NULL,
    purpose text NOT NULL,
    email text DEFAULT ''::text NOT NULL,
    created_at text DEFAULT ''::text NOT NULL,
    expires_at text DEFAULT ''::text NOT NULL,
    used_at text DEFAULT ''::text NOT NULL
);


--
-- Name: invite_uses; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.invite_uses (
    code text NOT NULL,
    telegram_id bigint NOT NULL,
    created_at text DEFAULT ''::text NOT NULL
);


--
-- Name: invites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.invites (
    code text NOT NULL,
    max_uses integer DEFAULT 1 NOT NULL,
    used integer DEFAULT 0 NOT NULL,
    expires_at text DEFAULT ''::text NOT NULL,
    created_at text DEFAULT ''::text NOT NULL,
    revoked integer DEFAULT 0 NOT NULL,
    note text DEFAULT ''::text NOT NULL
);


--
-- Name: invoice_snapshots; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.invoice_snapshots (
    telegram_id bigint NOT NULL,
    method text NOT NULL,
    months integer NOT NULL,
    plan_snapshot text DEFAULT ''::text NOT NULL,
    created_at text DEFAULT ''::text NOT NULL
);


--
-- Name: media_cache; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.media_cache (
    section text NOT NULL,
    file_id text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: p2p_requests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.p2p_requests (
    id bigint NOT NULL,
    telegram_id bigint NOT NULL,
    months integer NOT NULL,
    price text DEFAULT ''::text NOT NULL,
    status text NOT NULL,
    screenshot text DEFAULT ''::text NOT NULL,
    comment text DEFAULT ''::text NOT NULL,
    created_at text DEFAULT ''::text NOT NULL,
    decided_at text DEFAULT ''::text NOT NULL,
    plan_snapshot text DEFAULT ''::text NOT NULL,
    card text DEFAULT ''::text NOT NULL,
    purpose text DEFAULT ''::text NOT NULL,
    kopecks bigint DEFAULT 0 NOT NULL
);


--
-- Name: payment_log; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.payment_log (
    id bigint NOT NULL,
    ext_id text DEFAULT ''::text NOT NULL,
    telegram_id bigint DEFAULT 0 NOT NULL,
    method text DEFAULT ''::text NOT NULL,
    stage text DEFAULT ''::text NOT NULL,
    detail text DEFAULT ''::text NOT NULL,
    created_at text DEFAULT ''::text NOT NULL
);


--
-- Name: payments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.payments (
    id bigint NOT NULL,
    telegram_id bigint NOT NULL,
    method text NOT NULL,
    months integer NOT NULL,
    amount text DEFAULT ''::text NOT NULL,
    status text NOT NULL,
    comment text DEFAULT ''::text NOT NULL,
    created_at text DEFAULT ''::text NOT NULL,
    ext_id text DEFAULT ''::text NOT NULL,
    plan_snapshot text DEFAULT ''::text NOT NULL
);


--
-- Name: pending_invoices; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.pending_invoices (
    id bigint NOT NULL,
    method text NOT NULL,
    ext_id text NOT NULL,
    telegram_id bigint NOT NULL,
    months integer NOT NULL,
    created_at text DEFAULT ''::text NOT NULL,
    resolved integer DEFAULT 0 NOT NULL,
    purpose text DEFAULT ''::text NOT NULL,
    kopecks bigint DEFAULT 0 NOT NULL,
    plan_snapshot text DEFAULT ''::text NOT NULL
);


--
-- Name: plan_access; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plan_access (
    plan_code text NOT NULL,
    telegram_id bigint DEFAULT 0 NOT NULL,
    email text DEFAULT ''::text NOT NULL,
    created_at text DEFAULT ''::text NOT NULL
);


--
-- Name: plans; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plans (
    code text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    icon text DEFAULT ''::text NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    enabled integer DEFAULT 0 NOT NULL,
    traffic_gb integer DEFAULT 0 NOT NULL,
    device_limit integer DEFAULT 0 NOT NULL,
    strategy text DEFAULT ''::text NOT NULL,
    int_squads text DEFAULT ''::text NOT NULL,
    ext_squad text DEFAULT ''::text NOT NULL,
    availability text DEFAULT 'all'::text NOT NULL,
    currency text DEFAULT ''::text NOT NULL,
    durations text DEFAULT ''::text NOT NULL,
    from_config integer DEFAULT 0 NOT NULL,
    created_at text DEFAULT ''::text NOT NULL,
    updated_at text DEFAULT ''::text NOT NULL,
    addsub text DEFAULT ''::text NOT NULL,
    addsub_name text DEFAULT ''::text NOT NULL,
    addsub_desc text DEFAULT ''::text NOT NULL,
    addsub_traffic_gb integer DEFAULT '-1'::integer NOT NULL,
    addsub_squads text DEFAULT ''::text NOT NULL
);


--
-- Name: promo_codes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.promo_codes (
    code text NOT NULL,
    kind text NOT NULL,
    value integer DEFAULT 0 NOT NULL,
    max_uses integer DEFAULT 0 NOT NULL,
    used integer DEFAULT 0 NOT NULL,
    expires_at text DEFAULT ''::text NOT NULL,
    created_at text DEFAULT ''::text NOT NULL
);


--
-- Name: promo_redemptions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.promo_redemptions (
    code text NOT NULL,
    telegram_id bigint NOT NULL,
    created_at text DEFAULT ''::text NOT NULL
);


--
-- Name: purchase_intents; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.purchase_intents (
    telegram_id bigint NOT NULL,
    plan_code text DEFAULT ''::text NOT NULL,
    months integer DEFAULT 0 NOT NULL,
    days integer DEFAULT 0 NOT NULL,
    created_at text DEFAULT ''::text NOT NULL,
    shown_price text DEFAULT ''::text NOT NULL
);


--
-- Name: schema_migrations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.schema_migrations (
    version integer NOT NULL
);


--
-- Name: screen_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.screen_state (
    chat_id bigint NOT NULL,
    msg_id integer NOT NULL
);


--
-- Name: settings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.settings (
    id integer NOT NULL,
    config text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: torrent_reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.torrent_reports (
    id bigint NOT NULL,
    telegram_id bigint DEFAULT 0 NOT NULL,
    username text DEFAULT ''::text NOT NULL,
    node text DEFAULT ''::text NOT NULL,
    ip text DEFAULT ''::text NOT NULL,
    protocol text DEFAULT ''::text NOT NULL,
    inbound text DEFAULT ''::text NOT NULL,
    source text DEFAULT ''::text NOT NULL,
    destination text DEFAULT ''::text NOT NULL,
    block_seconds integer DEFAULT 0 NOT NULL,
    will_unblock_at text DEFAULT ''::text NOT NULL,
    unblock_notified integer DEFAULT 0 NOT NULL,
    created_at text DEFAULT ''::text NOT NULL
);


--
-- Name: torrent_strikes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.torrent_strikes (
    tg_id bigint NOT NULL,
    struck_at text NOT NULL
);


--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users (
    telegram_id bigint NOT NULL,
    p2p_approved integer DEFAULT 0 NOT NULL,
    created_at text DEFAULT ''::text NOT NULL,
    blocked integer DEFAULT 0 NOT NULL,
    username text DEFAULT ''::text NOT NULL,
    first_name text DEFAULT ''::text NOT NULL,
    terms_accepted_at timestamp with time zone,
    trial_used_at timestamp with time zone,
    sub_expire_at text DEFAULT ''::text NOT NULL,
    notify_kind text DEFAULT ''::text NOT NULL,
    notify_sent text DEFAULT ''::text NOT NULL,
    balance bigint DEFAULT 0 NOT NULL,
    referred_by bigint DEFAULT 0 NOT NULL,
    ref_bonus_paid integer DEFAULT 0 NOT NULL,
    whitelisted integer DEFAULT 0 NOT NULL,
    ref_earned bigint DEFAULT 0 NOT NULL,
    web_approved integer DEFAULT 0 NOT NULL,
    web_denied integer DEFAULT 0 NOT NULL,
    plan_snapshot text DEFAULT ''::text NOT NULL,
    unreachable_at text DEFAULT ''::text NOT NULL,
    trial_resets integer DEFAULT 0 NOT NULL,
    traffic_bonus text DEFAULT ''::text NOT NULL,
    sess_epoch integer DEFAULT 0 NOT NULL,
    ref_on_pay integer DEFAULT 0 NOT NULL
);


--
-- Name: web_users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.web_users (
    tg_id bigint NOT NULL,
    email text NOT NULL,
    pass_hash text NOT NULL,
    created_at text DEFAULT ''::text NOT NULL,
    email_verified_at text DEFAULT ''::text NOT NULL,
    prev_tg_id bigint DEFAULT 0 NOT NULL
);


--
-- Name: whitelist; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.whitelist (
    telegram_id bigint NOT NULL,
    created_at text DEFAULT ''::text NOT NULL
);


--
-- Data for Name: autopay; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.autopay (telegram_id, method, method_id, title, months, amount, currency, enabled, created_at, last_pay_at, next_try_at, fails, last_error, paid_period, plan_snapshot) FROM stdin;
5001	yookassa	pm-fake-1	Карта •• 4242	1	199.00	RUB	1	2026-02-01T10:06:00Z			0			{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"strategy":"MONTH","int_squads":["11111111-1111-4111-8111-111111111111"],"price":"199","currency":"RUB"}
\.


--
-- Data for Name: backup_state; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.backup_state (id, data) FROM stdin;
\.


--
-- Data for Name: email_tokens; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.email_tokens (hash, tg_id, purpose, email, created_at, expires_at, used_at) FROM stdin;
\.


--
-- Data for Name: invite_uses; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.invite_uses (code, telegram_id, created_at) FROM stdin;
\.


--
-- Data for Name: invites; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.invites (code, max_uses, used, expires_at, created_at, revoked, note) FROM stdin;
\.


--
-- Data for Name: invoice_snapshots; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.invoice_snapshots (telegram_id, method, months, plan_snapshot, created_at) FROM stdin;
\.


--
-- Data for Name: media_cache; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.media_cache (section, file_id, updated_at) FROM stdin;
\.


--
-- Data for Name: p2p_requests; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.p2p_requests (id, telegram_id, months, price, status, screenshot, comment, created_at, decided_at, plan_snapshot, card, purpose, kopecks) FROM stdin;
31	5003	1	199	awaiting			2026-09-01T00:00:00Z					0
\.


--
-- Data for Name: payment_log; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.payment_log (id, ext_id, telegram_id, method, stage, detail, created_at) FROM stdin;
\.


--
-- Data for Name: payments; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.payments (id, telegram_id, method, months, amount, status, comment, created_at, ext_id, plan_snapshot) FROM stdin;
1	5001	yookassa	1	199.00 RUB	paid		2026-02-01T10:06:00Z	yk-1	{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"strategy":"MONTH","int_squads":["11111111-1111-4111-8111-111111111111"],"price":"199","currency":"RUB"}
2	5001	stars	1	100 ⭐	paid		2026-03-01T10:06:00Z	chg-2	{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"strategy":"MONTH","int_squads":["11111111-1111-4111-8111-111111111111"],"price":"199","currency":"RUB"}
3	5002	p2p	3	450 ₽	paid		2026-03-02T11:10:00Z	p2p:3	{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"strategy":"MONTH","int_squads":["11111111-1111-4111-8111-111111111111"],"price":"199","currency":"RUB"}
4	5001	yookassa	0	500.00 RUB	paid	topup	2026-04-01T10:06:00Z	top-4	
5	5002	cryptobot	1	2.5 USDT	paid		2026-04-02T11:10:00Z	cb-5	{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"price":"199","currency":"RUB","paid":"2.5 USDT"}
6	5005	heleket	6	990.00	paid		2026-05-05T14:10:00Z	hl-6	{"code":"pro","name":"Про","months":6,"traffic_gb":0,"device_limit":5,"strategy":"NO_RESET","int_squads":["11111111-1111-4111-8111-111111111111","22222222-2222-4222-8222-222222222222"],"addsub":true,"price":"990","currency":"RUB"}
7	5001	balance	1	199 ₽	paid		2026-05-01T10:06:00Z		{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"strategy":"MONTH","int_squads":["11111111-1111-4111-8111-111111111111"],"price":"199","currency":"RUB"}
8	5002	yookassa	1	199.00 RUB	refunded		2026-05-02T11:10:00Z	yk-8	{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"strategy":"MONTH","int_squads":["11111111-1111-4111-8111-111111111111"],"price":"199","currency":"RUB"}
9	5003	yookassa	1	199.00 RUB	rejected		2026-05-03T12:10:00Z	yk-9	
10	5004	trial	0		paid		2026-03-04T13:05:00Z		
11	-7001	platega	1	199.00 RUB	paid		2026-06-06T15:10:00Z	pl-11	{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"strategy":"MONTH","int_squads":["11111111-1111-4111-8111-111111111111"],"price":"199","currency":"RUB"}
12	5005	tribute	6	990 RUB	paid		2026-08-05T14:10:00Z	tr-12	{"code":"pro","name":"Про","months":6,"traffic_gb":0,"device_limit":5,"strategy":"NO_RESET","int_squads":["11111111-1111-4111-8111-111111111111","22222222-2222-4222-8222-222222222222"],"addsub":true,"price":"990","currency":"RUB"}
\.


--
-- Data for Name: pending_invoices; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.pending_invoices (id, method, ext_id, telegram_id, months, created_at, resolved, purpose, kopecks, plan_snapshot) FROM stdin;
\.


--
-- Data for Name: plan_access; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.plan_access (plan_code, telegram_id, email, created_at) FROM stdin;
pro	5005		2026-05-05T14:00:00Z
\.


--
-- Data for Name: plans; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.plans (code, name, description, icon, sort_order, enabled, traffic_gb, device_limit, strategy, int_squads, ext_squad, availability, currency, durations, from_config, created_at, updated_at, addsub, addsub_name, addsub_desc, addsub_traffic_gb, addsub_squads) FROM stdin;
std	Стандарт	Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. Подробное описание тарифа для проверки длинных строк. 		1	1	100	3	MONTH	["11111111-1111-4111-8111-111111111111"]		all	RUB	[{"months":1,"base":"199","stars":100},{"months":3,"base":"499"},{"months":12,"base":"1790","stars":900}]	1	2026-01-01T00:00:00Z	2026-01-01T00:00:00Z				-1	
pro	Про	Для семьи		2	1	0	5	NO_RESET	["11111111-1111-4111-8111-111111111111","22222222-2222-4222-8222-222222222222"]		list		[{"months":1,"base":"399"},{"months":6,"base":"1990"},{"days":7,"base":"129","traffic_gb":50}]	0	2026-01-02T00:00:00Z	2026-01-02T00:00:00Z	on	Второй аккаунт		50	
old	Старый			3	0	50	1	MONTH			all			0	2026-01-03T00:00:00Z	2026-01-03T00:00:00Z				-1	
\.


--
-- Data for Name: promo_codes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.promo_codes (code, kind, value, max_uses, used, expires_at, created_at) FROM stdin;
FREE7	days	7	100	1	2027-01-01T00:00:00Z	2026-01-05T00:00:00Z
CASH100	balance	10000	1	0		2026-01-06T00:00:00Z
GIGA	traffic	50	0	0		2026-01-07T00:00:00Z
Скидка	days	3	0	0		2026-01-08T00:00:00Z
\.


--
-- Data for Name: promo_redemptions; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.promo_redemptions (code, telegram_id, created_at) FROM stdin;
FREE7	5002	2026-03-05T10:00:00Z
\.


--
-- Data for Name: purchase_intents; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.purchase_intents (telegram_id, plan_code, months, days, created_at, shown_price) FROM stdin;
\.


--
-- Data for Name: schema_migrations; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.schema_migrations (version) FROM stdin;
1
2
3
4
5
6
7
8
9
10
11
12
13
14
15
16
17
18
19
20
21
22
23
24
25
26
27
28
29
30
31
32
33
34
35
36
37
38
39
40
41
42
43
44
45
46
47
48
49
50
51
52
53
54
55
56
57
58
59
60
61
62
63
64
65
66
67
68
69
70
71
72
73
74
75
76
77
\.


--
-- Data for Name: screen_state; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.screen_state (chat_id, msg_id) FROM stdin;
\.


--
-- Data for Name: settings; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.settings (id, config, updated_at) FROM stdin;
1	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	2026-11-18 14:54:53.655731+02
\.


--
-- Data for Name: torrent_reports; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.torrent_reports (id, telegram_id, username, node, ip, protocol, inbound, source, destination, block_seconds, will_unblock_at, unblock_notified, created_at) FROM stdin;
1	5002	tg_5002	NL-1	198.51.100.7	bittorrent	vless	198.51.100.7	203.0.113.9	600	2026-10-01T00:10:00Z	1	2026-10-01T00:00:00Z
\.


--
-- Data for Name: torrent_strikes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.torrent_strikes (tg_id, struck_at) FROM stdin;
\.


--
-- Data for Name: users; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.users (telegram_id, p2p_approved, created_at, blocked, username, first_name, terms_accepted_at, trial_used_at, sub_expire_at, notify_kind, notify_sent, balance, referred_by, ref_bonus_paid, whitelisted, ref_earned, web_approved, web_denied, plan_snapshot, unreachable_at, trial_resets, traffic_bonus, sess_epoch, ref_on_pay) FROM stdin;
5001	1	2026-02-01T10:00:00Z	0	katya	Катя	2026-02-01 12:01:00+02	2026-02-01 12:05:00+02	2026-12-28T10:00:00Z			25000	0	0	0	1990	0	0	{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"strategy":"MONTH","int_squads":["11111111-1111-4111-8111-111111111111"],"price":"199","currency":"RUB"}		0		0	0
5002	0	2026-03-02T11:00:00Z	1		Lev	\N	\N	2026-12-30T00:00:00Z			0	5001	1	0	0	0	0	{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"strategy":"MONTH","int_squads":["11111111-1111-4111-8111-111111111111"],"price":"199","currency":"RUB"}		0		0	0
5003	0	2026-04-03T12:00:00Z	0	oleg_v	Олег	\N	\N				1500	5001	0	0	0	0	0		2026-09-01T00:00:00Z	0		0	0
5004	0	2026-03-04T13:00:00Z	0	nina	Нина	\N	2026-03-04 15:05:00+02	2026-03-10T00:00:00Z			0	0	0	0	0	0	0			0		0	0
5005	0	2026-05-05T14:00:00Z	0	pavel	Павел	\N	\N	2027-02-01T00:00:00Z			0	5002	0	0	0	0	0	{"code":"pro","name":"Про","months":6,"traffic_gb":0,"device_limit":5,"strategy":"NO_RESET","int_squads":["11111111-1111-4111-8111-111111111111","22222222-2222-4222-8222-222222222222"],"addsub":true,"price":"990","currency":"RUB"}		0		0	0
-7001	0	2026-06-06T15:00:00Z	0			\N	\N	2027-01-05T00:00:00Z			700	0	0	0	0	0	0	{"code":"std","name":"Стандарт","months":1,"traffic_gb":100,"device_limit":3,"strategy":"MONTH","int_squads":["11111111-1111-4111-8111-111111111111"],"price":"199","currency":"RUB"}		0		0	0
\.


--
-- Data for Name: web_users; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.web_users (tg_id, email, pass_hash, created_at, email_verified_at, prev_tg_id) FROM stdin;
5001	Katya@Example.com	$2a$10$abcdefghijklmnopqrstuuOdGq1xQmB0PZ5d4fN4m0wE1gVyZcH3W	2026-02-02T10:00:00Z	2026-02-02T10:10:00Z	0
-7001	mira@example.com	$2a$10$zyxwvutsrqponmlkjihgfeOdGq1xQmB0PZ5d4fN4m0wE1gVyZcH3W	2026-06-06T15:00:00Z		0
\.


--
-- Data for Name: whitelist; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.whitelist (telegram_id, created_at) FROM stdin;
5001	2026-02-01T00:00:00Z
\.


--
-- Name: autopay autopay_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.autopay
    ADD CONSTRAINT autopay_pkey PRIMARY KEY (telegram_id);


--
-- Name: backup_state backup_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.backup_state
    ADD CONSTRAINT backup_state_pkey PRIMARY KEY (id);


--
-- Name: email_tokens email_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_tokens
    ADD CONSTRAINT email_tokens_pkey PRIMARY KEY (hash);


--
-- Name: invite_uses invite_uses_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invite_uses
    ADD CONSTRAINT invite_uses_pkey PRIMARY KEY (code, telegram_id);


--
-- Name: invites invites_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_pkey PRIMARY KEY (code);


--
-- Name: invoice_snapshots invoice_snapshots_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invoice_snapshots
    ADD CONSTRAINT invoice_snapshots_pkey PRIMARY KEY (telegram_id, method, months);


--
-- Name: media_cache media_cache_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.media_cache
    ADD CONSTRAINT media_cache_pkey PRIMARY KEY (section);


--
-- Name: p2p_requests p2p_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.p2p_requests
    ADD CONSTRAINT p2p_requests_pkey PRIMARY KEY (id);


--
-- Name: payment_log payment_log_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payment_log
    ADD CONSTRAINT payment_log_pkey PRIMARY KEY (id);


--
-- Name: payments payments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payments
    ADD CONSTRAINT payments_pkey PRIMARY KEY (id);


--
-- Name: pending_invoices pending_invoices_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.pending_invoices
    ADD CONSTRAINT pending_invoices_pkey PRIMARY KEY (id);


--
-- Name: plan_access plan_access_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plan_access
    ADD CONSTRAINT plan_access_pkey PRIMARY KEY (plan_code, telegram_id, email);


--
-- Name: plans plans_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plans
    ADD CONSTRAINT plans_pkey PRIMARY KEY (code);


--
-- Name: promo_codes promo_codes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.promo_codes
    ADD CONSTRAINT promo_codes_pkey PRIMARY KEY (code);


--
-- Name: promo_redemptions promo_redemptions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.promo_redemptions
    ADD CONSTRAINT promo_redemptions_pkey PRIMARY KEY (code, telegram_id);


--
-- Name: purchase_intents purchase_intents_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.purchase_intents
    ADD CONSTRAINT purchase_intents_pkey PRIMARY KEY (telegram_id);


--
-- Name: schema_migrations schema_migrations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.schema_migrations
    ADD CONSTRAINT schema_migrations_pkey PRIMARY KEY (version);


--
-- Name: screen_state screen_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screen_state
    ADD CONSTRAINT screen_state_pkey PRIMARY KEY (chat_id);


--
-- Name: settings settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.settings
    ADD CONSTRAINT settings_pkey PRIMARY KEY (id);


--
-- Name: torrent_reports torrent_reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.torrent_reports
    ADD CONSTRAINT torrent_reports_pkey PRIMARY KEY (id);


--
-- Name: torrent_strikes torrent_strikes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.torrent_strikes
    ADD CONSTRAINT torrent_strikes_pkey PRIMARY KEY (tg_id);


--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (telegram_id);


--
-- Name: web_users web_users_email_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.web_users
    ADD CONSTRAINT web_users_email_key UNIQUE (email);


--
-- Name: web_users web_users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.web_users
    ADD CONSTRAINT web_users_pkey PRIMARY KEY (tg_id);


--
-- Name: whitelist whitelist_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.whitelist
    ADD CONSTRAINT whitelist_pkey PRIMARY KEY (telegram_id);


--
-- Name: idx_email_tokens_owner; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_email_tokens_owner ON public.email_tokens USING btree (tg_id, purpose);


--
-- Name: idx_paylog_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_paylog_created ON public.payment_log USING btree (created_at);


--
-- Name: idx_paylog_ext; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_paylog_ext ON public.payment_log USING btree (ext_id);


--
-- Name: idx_paylog_stage; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_paylog_stage ON public.payment_log USING btree (stage);


--
-- Name: idx_paylog_tg; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_paylog_tg ON public.payment_log USING btree (telegram_id);


--
-- Name: idx_payments_user_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_payments_user_created ON public.payments USING btree (telegram_id, created_at);


--
-- Name: idx_plan_access_tg; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_plan_access_tg ON public.plan_access USING btree (telegram_id);


--
-- Name: idx_torrent_reports_due; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_torrent_reports_due ON public.torrent_reports USING btree (unblock_notified, will_unblock_at);


--
-- Name: idx_torrent_reports_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_torrent_reports_name ON public.torrent_reports USING btree (username, created_at);


--
-- Name: idx_torrent_reports_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_torrent_reports_user ON public.torrent_reports USING btree (telegram_id, created_at);


--
-- Name: uq_payments_method_extid; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX uq_payments_method_extid ON public.payments USING btree (method, ext_id) WHERE (ext_id <> ''::text);


--
-- PostgreSQL database dump complete
--

\unrestrict rz42Vh73BFn6oBeD9oUj5dF6X0FXJGfiSqhBOzgBENQKHMBZDkukkCg4Qf2Hlvl

