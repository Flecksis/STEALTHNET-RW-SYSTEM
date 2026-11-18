--
-- PostgreSQL database dump
--

\restrict U8gzgwANlymcLhtiCPvyCL1Fbh8kvlNgV8vV4fNb25Ve6IUHtRxd5HfOxSYYebi

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
-- Name: bot_menu_photo; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.bot_menu_photo (
    bot_id bigint NOT NULL,
    path text NOT NULL,
    content_hash text NOT NULL,
    file_id text NOT NULL
);


--
-- Name: bot_menu_pin; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.bot_menu_pin (
    bot_id bigint NOT NULL,
    chat_id bigint NOT NULL,
    message_id integer NOT NULL,
    CONSTRAINT bot_menu_pin_message_id_check CHECK ((message_id > 0))
);


--
-- Name: customer; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.customer (
    id bigint NOT NULL,
    telegram_id bigint,
    expire_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP,
    subscription_link text,
    language character varying(10) DEFAULT 'en'::character varying
);


--
-- Name: customer_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.customer_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: customer_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.customer_id_seq OWNED BY public.customer.id;


--
-- Name: purchase; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.purchase (
    id bigint NOT NULL,
    amount numeric(20,8) NOT NULL,
    customer_id bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP,
    month integer NOT NULL,
    paid_at timestamp with time zone,
    currency character varying(10),
    expire_at timestamp with time zone,
    status character varying(20),
    invoice_type character varying(20),
    crypto_invoice_id bigint,
    crypto_invoice_url text,
    yookasa_url text,
    yookasa_id uuid,
    platega_id text,
    platega_url text,
    wata_id text,
    wata_url text
);


--
-- Name: purchase_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.purchase_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: purchase_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.purchase_id_seq OWNED BY public.purchase.id;


--
-- Name: referral; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.referral (
    id bigint NOT NULL,
    referrer_id bigint NOT NULL,
    referee_id bigint NOT NULL,
    used_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP,
    bonus_granted boolean DEFAULT false NOT NULL
);


--
-- Name: referral_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.referral_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: referral_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.referral_id_seq OWNED BY public.referral.id;


--
-- Name: schema_migrations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.schema_migrations (
    version bigint NOT NULL,
    dirty boolean NOT NULL
);


--
-- Name: customer id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.customer ALTER COLUMN id SET DEFAULT nextval('public.customer_id_seq'::regclass);


--
-- Name: purchase id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.purchase ALTER COLUMN id SET DEFAULT nextval('public.purchase_id_seq'::regclass);


--
-- Name: referral id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.referral ALTER COLUMN id SET DEFAULT nextval('public.referral_id_seq'::regclass);


--
-- Data for Name: bot_menu_photo; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.bot_menu_photo (bot_id, path, content_hash, file_id) FROM stdin;
42	/assets/menu.jpg	abc	AgACAgfake
\.


--
-- Data for Name: bot_menu_pin; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.bot_menu_pin (bot_id, chat_id, message_id) FROM stdin;
\.


--
-- Data for Name: customer; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.customer (id, telegram_id, expire_at, created_at, subscription_link, language) FROM stdin;
1	3001	2026-12-20 12:00:00+02	2026-03-01 11:00:00+02	https://sub.example.com/rtsAAA111	ru
2	3002	2026-12-25 14:00:00+02	2026-04-02 12:00:00+03	https://sub.example.com/rtsBBB222	en
3	3003	\N	2026-05-03 12:00:00+03	\N	ru
4	3004	2026-10-01 03:00:00+03	2026-06-04 12:00:00+03	https://sub.example.com/rtsDDD444/	ru
5	3005	2027-01-15 10:30:00+02	2026-07-05 12:00:00+03	https://sub.example.com/rtsEEE555	ru
\.


--
-- Data for Name: purchase; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.purchase (id, amount, customer_id, created_at, month, paid_at, currency, expire_at, status, invoice_type, crypto_invoice_id, crypto_invoice_url, yookasa_url, yookasa_id, platega_id, platega_url, wata_id, wata_url) FROM stdin;
1	199.00000000	1	2026-03-01 11:05:00+02	1	2026-03-01 11:06:00+02	RUB	2026-03-01 12:05:00+02	paid	yookasa	\N	\N	https://yoomoney.example/checkout/1	2f6c7a90-0000-4000-8000-000000000001	\N	\N	\N	\N
2	499.00000000	1	2026-04-01 12:05:00+03	3	2026-04-01 12:07:00+03	RUB	2026-04-01 13:05:00+03	paid	crypto	777	https://t.me/CryptoBot?start=IVfake	\N	\N	\N	\N	\N	\N
3	150.00000000	2	2026-04-02 12:10:00+03	1	2026-04-02 12:11:00+03	STARS	\N	paid	telegram	\N	\N	\N	\N	\N	\N	\N	\N
4	199.00000000	2	2026-05-02 12:10:00+03	1	\N	RUB	2026-05-02 13:10:00+03	new	yookasa	\N	\N	https://yoomoney.example/checkout/4	2f6c7a90-0000-4000-8000-000000000004	\N	\N	\N	\N
5	999.00000000	5	2026-07-05 12:20:00+03	6	2026-07-05 12:22:00+03	RUB	\N	paid	plt_sbp	\N	\N	\N	\N	pl-5	https://pay.example/pl-5	\N	\N
6	1990.00000000	5	2026-08-05 12:20:00+03	12	\N	RUB	\N	cancel	platega	\N	\N	\N	\N	pl-6	https://pay.example/pl-6	\N	\N
7	199.00000000	1	2026-06-01 12:05:00+03	1	2026-06-01 12:06:00+03	RUB	\N	paid	tribute	\N	\N	\N	\N	\N	\N	\N	\N
8	299.50000000	5	2026-09-05 12:20:00+03	1	2026-09-05 12:25:00+03	RUB	\N	paid	wata	\N	\N	\N	\N	\N	\N	wt-8	https://pay.example/wt-8
9	0.00000000	4	2026-06-04 12:30:00+03	1	2026-06-04 12:30:00+03	RUB	\N	paid	yookasa	\N	\N	\N	\N	\N	\N	\N	\N
\.


--
-- Data for Name: referral; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.referral (id, referrer_id, referee_id, used_at, bonus_granted) FROM stdin;
1	3001	3002	2026-04-02 12:00:00+03	t
2	3002	3005	2026-07-05 12:00:00+03	f
\.


--
-- Data for Name: schema_migrations; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.schema_migrations (version, dirty) FROM stdin;
7	f
\.


--
-- Name: customer_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.customer_id_seq', 5, true);


--
-- Name: purchase_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.purchase_id_seq', 9, true);


--
-- Name: referral_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.referral_id_seq', 2, true);


--
-- Name: bot_menu_photo bot_menu_photo_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bot_menu_photo
    ADD CONSTRAINT bot_menu_photo_pkey PRIMARY KEY (bot_id, path);


--
-- Name: bot_menu_pin bot_menu_pin_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bot_menu_pin
    ADD CONSTRAINT bot_menu_pin_pkey PRIMARY KEY (bot_id, chat_id, message_id);


--
-- Name: customer customer_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.customer
    ADD CONSTRAINT customer_pkey PRIMARY KEY (id);


--
-- Name: customer customer_telegram_id_unique; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.customer
    ADD CONSTRAINT customer_telegram_id_unique UNIQUE (telegram_id);


--
-- Name: purchase purchase_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.purchase
    ADD CONSTRAINT purchase_pkey PRIMARY KEY (id);


--
-- Name: referral referral_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.referral
    ADD CONSTRAINT referral_pkey PRIMARY KEY (id);


--
-- Name: schema_migrations schema_migrations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.schema_migrations
    ADD CONSTRAINT schema_migrations_pkey PRIMARY KEY (version);


--
-- Name: idx_customer_telegram_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_customer_telegram_id ON public.customer USING hash (telegram_id);


--
-- Name: idx_referral_referee_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_referral_referee_id ON public.referral USING hash (referee_id);


--
-- Name: idx_referral_referrer_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_referral_referrer_id ON public.referral USING hash (referrer_id);


--
-- Name: purchase purchase_customer_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.purchase
    ADD CONSTRAINT purchase_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES public.customer(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;


--
-- Name: referral referral_referee_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.referral
    ADD CONSTRAINT referral_referee_id_fkey FOREIGN KEY (referee_id) REFERENCES public.customer(telegram_id) ON DELETE CASCADE;


--
-- Name: referral referral_referrer_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.referral
    ADD CONSTRAINT referral_referrer_id_fkey FOREIGN KEY (referrer_id) REFERENCES public.customer(telegram_id) ON DELETE CASCADE;


--
-- PostgreSQL database dump complete
--

\unrestrict U8gzgwANlymcLhtiCPvyCL1Fbh8kvlNgV8vV4fNb25Ve6IUHtRxd5HfOxSYYebi

