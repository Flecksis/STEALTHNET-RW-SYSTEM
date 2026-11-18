--
-- PostgreSQL database dump
--

\restrict 1xjNygS13o1XvuENssevIhLS1KMmASs8cAJjtGQC8aaAdZuaChQ0PrTvqip7jyg

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

--
-- Name: broadcast_audience; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.broadcast_audience AS ENUM (
    'ALL',
    'PLAN',
    'SUBSCRIBED',
    'UNSUBSCRIBED',
    'EXPIRED',
    'TRIAL'
);


ALTER TYPE public.broadcast_audience OWNER TO postgres;

--
-- Name: broadcast_message_status; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.broadcast_message_status AS ENUM (
    'SENT',
    'FAILED',
    'EDITED',
    'DELETED',
    'PENDING'
);


ALTER TYPE public.broadcast_message_status OWNER TO postgres;

--
-- Name: broadcast_status; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.broadcast_status AS ENUM (
    'PROCESSING',
    'COMPLETED',
    'CANCELED',
    'DELETED',
    'ERROR'
);


ALTER TYPE public.broadcast_status OWNER TO postgres;

--
-- Name: currency; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.currency AS ENUM (
    'USD',
    'XTR',
    'RUB'
);


ALTER TYPE public.currency OWNER TO postgres;

--
-- Name: locale; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.locale AS ENUM (
    'AR',
    'AZ',
    'BE',
    'CS',
    'DE',
    'EN',
    'ES',
    'FA',
    'FR',
    'HE',
    'HI',
    'ID',
    'IT',
    'JA',
    'KK',
    'KO',
    'MS',
    'NL',
    'PL',
    'PT',
    'RO',
    'RU',
    'SR',
    'TR',
    'UK',
    'UZ',
    'VI',
    'BG',
    'DA',
    'EL',
    'FI',
    'HU',
    'NO',
    'SV',
    'TG',
    'TH',
    'ZH'
);


ALTER TYPE public.locale OWNER TO postgres;

--
-- Name: payment_gateway_type; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.payment_gateway_type AS ENUM (
    'TELEGRAM_STARS',
    'YOOKASSA',
    'YOOMONEY',
    'CRYPTOMUS',
    'HELEKET',
    'CRYPTOPAY',
    'FREEKASSA',
    'MULENPAY',
    'PAYMASTER',
    'PLATEGA',
    'ROBOKASSA',
    'URLPAY',
    'WATA',
    'VALUTIX'
);


ALTER TYPE public.payment_gateway_type OWNER TO postgres;

--
-- Name: plan_availability; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.plan_availability AS ENUM (
    'ALL',
    'NEW',
    'EXISTING',
    'INVITED',
    'ALLOWED',
    'LINK'
);


ALTER TYPE public.plan_availability OWNER TO postgres;

--
-- Name: plan_traffic_limit_strategy; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.plan_traffic_limit_strategy AS ENUM (
    'NO_RESET',
    'DAY',
    'WEEK',
    'MONTH',
    'MONTH_ROLLING'
);


ALTER TYPE public.plan_traffic_limit_strategy OWNER TO postgres;

--
-- Name: plan_type; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.plan_type AS ENUM (
    'TRAFFIC',
    'DEVICES',
    'BOTH',
    'UNLIMITED'
);


ALTER TYPE public.plan_type OWNER TO postgres;

--
-- Name: promocode_availability; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.promocode_availability AS ENUM (
    'ALL',
    'NEW',
    'EXISTING',
    'INVITED'
);


ALTER TYPE public.promocode_availability OWNER TO postgres;

--
-- Name: promocode_reward_type; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.promocode_reward_type AS ENUM (
    'DURATION',
    'TRAFFIC',
    'DEVICES',
    'SUBSCRIPTION',
    'PERSONAL_DISCOUNT',
    'PURCHASE_DISCOUNT'
);


ALTER TYPE public.promocode_reward_type OWNER TO postgres;

--
-- Name: purchase_type; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.purchase_type AS ENUM (
    'NEW',
    'RENEW',
    'CHANGE'
);


ALTER TYPE public.purchase_type OWNER TO postgres;

--
-- Name: referral_level; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.referral_level AS ENUM (
    'FIRST',
    'SECOND'
);


ALTER TYPE public.referral_level OWNER TO postgres;

--
-- Name: referral_reward_type; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.referral_reward_type AS ENUM (
    'POINTS',
    'EXTRA_DAYS'
);


ALTER TYPE public.referral_reward_type OWNER TO postgres;

--
-- Name: subscription_status; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.subscription_status AS ENUM (
    'ACTIVE',
    'DISABLED',
    'LIMITED',
    'EXPIRED',
    'DELETED'
);


ALTER TYPE public.subscription_status OWNER TO postgres;

--
-- Name: traffic_limit_strategy; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.traffic_limit_strategy AS ENUM (
    'NO_RESET',
    'DAY',
    'WEEK',
    'MONTH'
);


ALTER TYPE public.traffic_limit_strategy OWNER TO postgres;

--
-- Name: transaction_status; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.transaction_status AS ENUM (
    'PENDING',
    'COMPLETED',
    'CANCELED',
    'REFUNDED',
    'FAILED'
);


ALTER TYPE public.transaction_status OWNER TO postgres;

--
-- Name: user_role; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.user_role AS ENUM (
    'DEV',
    'ADMIN',
    'USER',
    'PREVIEW',
    'OWNER',
    'SYSTEM'
);


ALTER TYPE public.user_role OWNER TO postgres;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: ad_links; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.ad_links (
    id integer NOT NULL,
    name character varying(128) NOT NULL,
    code character varying(64) NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL
);


ALTER TABLE public.ad_links OWNER TO postgres;

--
-- Name: ad_links_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.ad_links_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.ad_links_id_seq OWNER TO postgres;

--
-- Name: ad_links_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.ad_links_id_seq OWNED BY public.ad_links.id;


--
-- Name: alembic_version; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.alembic_version (
    version_num character varying(32) NOT NULL
);


ALTER TABLE public.alembic_version OWNER TO postgres;

--
-- Name: broadcast_messages; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.broadcast_messages (
    id integer NOT NULL,
    broadcast_id integer NOT NULL,
    user_telegram_id bigint,
    message_id bigint,
    status public.broadcast_message_status NOT NULL,
    user_id integer NOT NULL
);


ALTER TABLE public.broadcast_messages OWNER TO postgres;

--
-- Name: broadcast_messages_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.broadcast_messages_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.broadcast_messages_id_seq OWNER TO postgres;

--
-- Name: broadcast_messages_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.broadcast_messages_id_seq OWNED BY public.broadcast_messages.id;


--
-- Name: broadcasts; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.broadcasts (
    id integer NOT NULL,
    task_id uuid NOT NULL,
    status public.broadcast_status NOT NULL,
    audience public.broadcast_audience NOT NULL,
    total_count integer NOT NULL,
    success_count integer NOT NULL,
    failed_count integer NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL
);


ALTER TABLE public.broadcasts OWNER TO postgres;

--
-- Name: broadcasts_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.broadcasts_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.broadcasts_id_seq OWNER TO postgres;

--
-- Name: broadcasts_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.broadcasts_id_seq OWNED BY public.broadcasts.id;


--
-- Name: payment_gateways; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.payment_gateways (
    id integer NOT NULL,
    order_index integer NOT NULL,
    type public.payment_gateway_type NOT NULL,
    currency public.currency NOT NULL,
    is_active boolean NOT NULL,
    settings jsonb
);


ALTER TABLE public.payment_gateways OWNER TO postgres;

--
-- Name: payment_gateways_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.payment_gateways_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.payment_gateways_id_seq OWNER TO postgres;

--
-- Name: payment_gateways_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.payment_gateways_id_seq OWNED BY public.payment_gateways.id;


--
-- Name: plan_durations; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.plan_durations (
    id integer NOT NULL,
    days integer NOT NULL,
    plan_id integer NOT NULL,
    order_index integer NOT NULL
);


ALTER TABLE public.plan_durations OWNER TO postgres;

--
-- Name: plan_durations_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.plan_durations_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.plan_durations_id_seq OWNER TO postgres;

--
-- Name: plan_durations_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.plan_durations_id_seq OWNED BY public.plan_durations.id;


--
-- Name: plan_prices; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.plan_prices (
    id integer NOT NULL,
    currency public.currency NOT NULL,
    price numeric(10,2) NOT NULL,
    plan_duration_id integer NOT NULL
);


ALTER TABLE public.plan_prices OWNER TO postgres;

--
-- Name: plan_prices_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.plan_prices_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.plan_prices_id_seq OWNER TO postgres;

--
-- Name: plan_prices_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.plan_prices_id_seq OWNED BY public.plan_prices.id;


--
-- Name: plans; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.plans (
    id integer NOT NULL,
    order_index integer NOT NULL,
    is_active boolean NOT NULL,
    type public.plan_type NOT NULL,
    availability public.plan_availability NOT NULL,
    name character varying NOT NULL,
    traffic_limit integer NOT NULL,
    device_limit integer NOT NULL,
    allowed_telegram_ids bigint[] DEFAULT '{}'::bigint[] NOT NULL,
    internal_squads uuid[] NOT NULL,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    description character varying,
    tag character varying,
    traffic_limit_strategy public.plan_traffic_limit_strategy DEFAULT 'NO_RESET'::public.plan_traffic_limit_strategy NOT NULL,
    external_squad uuid,
    is_trial boolean NOT NULL,
    public_code character varying NOT NULL,
    allowed_emails text[] DEFAULT '{}'::text[] NOT NULL
);


ALTER TABLE public.plans OWNER TO postgres;

--
-- Name: plans_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.plans_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.plans_id_seq OWNER TO postgres;

--
-- Name: plans_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.plans_id_seq OWNED BY public.plans.id;


--
-- Name: promocode_activations; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.promocode_activations (
    id integer NOT NULL,
    promocode_id integer NOT NULL,
    user_id integer NOT NULL,
    activated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL
);


ALTER TABLE public.promocode_activations OWNER TO postgres;

--
-- Name: promocode_activations_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.promocode_activations_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.promocode_activations_id_seq OWNER TO postgres;

--
-- Name: promocode_activations_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.promocode_activations_id_seq OWNED BY public.promocode_activations.id;


--
-- Name: promocodes; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.promocodes (
    id integer NOT NULL,
    code character varying NOT NULL,
    is_active boolean NOT NULL,
    reward_type public.promocode_reward_type NOT NULL,
    reward integer,
    plan_snapshot jsonb,
    availability public.promocode_availability NOT NULL,
    expires_at timestamp with time zone,
    max_activations integer,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    is_reusable boolean DEFAULT false NOT NULL
);


ALTER TABLE public.promocodes OWNER TO postgres;

--
-- Name: promocodes_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.promocodes_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.promocodes_id_seq OWNER TO postgres;

--
-- Name: promocodes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.promocodes_id_seq OWNED BY public.promocodes.id;


--
-- Name: referral_rewards; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.referral_rewards (
    id integer NOT NULL,
    referral_id integer NOT NULL,
    type public.referral_reward_type NOT NULL,
    amount integer NOT NULL,
    is_issued boolean NOT NULL,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    user_id integer NOT NULL
);


ALTER TABLE public.referral_rewards OWNER TO postgres;

--
-- Name: referral_rewards_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.referral_rewards_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.referral_rewards_id_seq OWNER TO postgres;

--
-- Name: referral_rewards_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.referral_rewards_id_seq OWNED BY public.referral_rewards.id;


--
-- Name: referrals; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.referrals (
    id integer NOT NULL,
    level public.referral_level NOT NULL,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    referrer_id integer NOT NULL,
    referred_id integer NOT NULL
);


ALTER TABLE public.referrals OWNER TO postgres;

--
-- Name: referrals_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.referrals_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.referrals_id_seq OWNER TO postgres;

--
-- Name: referrals_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.referrals_id_seq OWNED BY public.referrals.id;


--
-- Name: settings; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.settings (
    id integer NOT NULL,
    default_currency public.currency NOT NULL,
    referral jsonb DEFAULT '{"enable": true, "level": 1, "accrual_strategy": "ON_FIRST_PAYMENT", "reward": {"type": "EXTRA_DAYS", "strategy": "AMOUNT", "config": {"1": 5}}}'::json NOT NULL,
    access jsonb NOT NULL,
    requirements jsonb NOT NULL,
    notifications jsonb NOT NULL,
    menu jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    backup jsonb NOT NULL,
    blacklist jsonb NOT NULL,
    extra jsonb DEFAULT '{}'::jsonb NOT NULL
);


ALTER TABLE public.settings OWNER TO postgres;

--
-- Name: settings_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.settings_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.settings_id_seq OWNER TO postgres;

--
-- Name: settings_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.settings_id_seq OWNED BY public.settings.id;


--
-- Name: subscriptions; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.subscriptions (
    id integer NOT NULL,
    user_remna_id uuid NOT NULL,
    status public.subscription_status NOT NULL,
    is_trial boolean NOT NULL,
    traffic_limit integer NOT NULL,
    device_limit integer NOT NULL,
    internal_squads uuid[] NOT NULL,
    expire_at timestamp with time zone NOT NULL,
    url character varying NOT NULL,
    plan_snapshot jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    external_squad uuid,
    traffic_limit_strategy public.plan_traffic_limit_strategy NOT NULL,
    tag character varying,
    user_id integer NOT NULL,
    device_single_reset_at timestamp with time zone,
    device_all_reset_at timestamp with time zone,
    link_reset_at timestamp with time zone,
    disabled_by_channel_leave boolean DEFAULT false NOT NULL
);


ALTER TABLE public.subscriptions OWNER TO postgres;

--
-- Name: subscriptions_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.subscriptions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.subscriptions_id_seq OWNER TO postgres;

--
-- Name: subscriptions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.subscriptions_id_seq OWNED BY public.subscriptions.id;


--
-- Name: transactions; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.transactions (
    id integer NOT NULL,
    payment_id uuid NOT NULL,
    status public.transaction_status NOT NULL,
    is_test boolean NOT NULL,
    purchase_type public.purchase_type NOT NULL,
    gateway_type public.payment_gateway_type NOT NULL,
    pricing jsonb NOT NULL,
    currency public.currency NOT NULL,
    plan_snapshot jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    user_id integer NOT NULL,
    gateway_display_name character varying,
    payment_method character varying
);


ALTER TABLE public.transactions OWNER TO postgres;

--
-- Name: transactions_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.transactions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.transactions_id_seq OWNER TO postgres;

--
-- Name: transactions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.transactions_id_seq OWNED BY public.transactions.id;


--
-- Name: user_oauth_providers; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.user_oauth_providers (
    id integer NOT NULL,
    user_id integer NOT NULL,
    provider character varying(32) NOT NULL,
    provider_id character varying(255) NOT NULL,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL
);


ALTER TABLE public.user_oauth_providers OWNER TO postgres;

--
-- Name: user_oauth_providers_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.user_oauth_providers_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.user_oauth_providers_id_seq OWNER TO postgres;

--
-- Name: user_oauth_providers_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.user_oauth_providers_id_seq OWNED BY public.user_oauth_providers.id;


--
-- Name: users; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.users (
    id integer NOT NULL,
    telegram_id bigint,
    username character varying(32),
    name character varying NOT NULL,
    role public.user_role NOT NULL,
    language public.locale NOT NULL,
    personal_discount integer NOT NULL,
    purchase_discount integer NOT NULL,
    is_blocked boolean NOT NULL,
    is_bot_blocked boolean NOT NULL,
    current_subscription_id integer,
    created_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    updated_at timestamp with time zone DEFAULT timezone('UTC'::text, now()) NOT NULL,
    referral_code character varying(64) NOT NULL,
    points integer DEFAULT 0 NOT NULL,
    is_rules_accepted boolean NOT NULL,
    is_trial_available boolean NOT NULL,
    email character varying(255),
    password_hash character varying(512),
    is_email_verified boolean NOT NULL,
    pending_email character varying(255),
    email_verification_code_hash character varying(128),
    email_verification_expires_at timestamp with time zone,
    auth_type character varying(20) NOT NULL,
    ad_link_id integer,
    referral_code_reset_at timestamp with time zone
);


ALTER TABLE public.users OWNER TO postgres;

--
-- Name: users_id_seq; Type: SEQUENCE; Schema: public; Owner: postgres
--

CREATE SEQUENCE public.users_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE public.users_id_seq OWNER TO postgres;

--
-- Name: users_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: postgres
--

ALTER SEQUENCE public.users_id_seq OWNED BY public.users.id;


--
-- Name: ad_links id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.ad_links ALTER COLUMN id SET DEFAULT nextval('public.ad_links_id_seq'::regclass);


--
-- Name: broadcast_messages id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.broadcast_messages ALTER COLUMN id SET DEFAULT nextval('public.broadcast_messages_id_seq'::regclass);


--
-- Name: broadcasts id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.broadcasts ALTER COLUMN id SET DEFAULT nextval('public.broadcasts_id_seq'::regclass);


--
-- Name: payment_gateways id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.payment_gateways ALTER COLUMN id SET DEFAULT nextval('public.payment_gateways_id_seq'::regclass);


--
-- Name: plan_durations id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.plan_durations ALTER COLUMN id SET DEFAULT nextval('public.plan_durations_id_seq'::regclass);


--
-- Name: plan_prices id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.plan_prices ALTER COLUMN id SET DEFAULT nextval('public.plan_prices_id_seq'::regclass);


--
-- Name: plans id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.plans ALTER COLUMN id SET DEFAULT nextval('public.plans_id_seq'::regclass);


--
-- Name: promocode_activations id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.promocode_activations ALTER COLUMN id SET DEFAULT nextval('public.promocode_activations_id_seq'::regclass);


--
-- Name: promocodes id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.promocodes ALTER COLUMN id SET DEFAULT nextval('public.promocodes_id_seq'::regclass);


--
-- Name: referral_rewards id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.referral_rewards ALTER COLUMN id SET DEFAULT nextval('public.referral_rewards_id_seq'::regclass);


--
-- Name: referrals id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.referrals ALTER COLUMN id SET DEFAULT nextval('public.referrals_id_seq'::regclass);


--
-- Name: settings id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.settings ALTER COLUMN id SET DEFAULT nextval('public.settings_id_seq'::regclass);


--
-- Name: subscriptions id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.subscriptions ALTER COLUMN id SET DEFAULT nextval('public.subscriptions_id_seq'::regclass);


--
-- Name: transactions id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.transactions ALTER COLUMN id SET DEFAULT nextval('public.transactions_id_seq'::regclass);


--
-- Name: user_oauth_providers id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.user_oauth_providers ALTER COLUMN id SET DEFAULT nextval('public.user_oauth_providers_id_seq'::regclass);


--
-- Name: users id; Type: DEFAULT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.users ALTER COLUMN id SET DEFAULT nextval('public.users_id_seq'::regclass);


--
-- Data for Name: ad_links; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.ad_links (id, name, code, is_active, created_at, updated_at) FROM stdin;
1	Канал про VPN	vpnchan	t	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02
\.


--
-- Data for Name: alembic_version; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.alembic_version (version_num) FROM stdin;
0040
\.


--
-- Data for Name: broadcast_messages; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.broadcast_messages (id, broadcast_id, user_telegram_id, message_id, status, user_id) FROM stdin;
\.


--
-- Data for Name: broadcasts; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.broadcasts (id, task_id, status, audience, total_count, success_count, failed_count, payload, created_at, updated_at) FROM stdin;
\.


--
-- Data for Name: payment_gateways; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.payment_gateways (id, order_index, type, currency, is_active, settings) FROM stdin;
1	1	TELEGRAM_STARS	XTR	t	{}
2	2	YOOKASSA	RUB	t	{"api_key": "fixture-not-a-key", "shop_id": "000000"}
3	3	CRYPTOPAY	USD	t	{"api_key": "fixture-not-a-key"}
4	4	HELEKET	USD	f	{}
5	5	PLATEGA	RUB	f	{}
\.


--
-- Data for Name: plan_durations; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.plan_durations (id, days, plan_id, order_index) FROM stdin;
1	30	1	1
2	90	1	2
3	0	1	3
4	3	2	1
5	30	3	1
\.


--
-- Data for Name: plan_prices; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.plan_prices (id, currency, price, plan_duration_id) FROM stdin;
1	RUB	199.00	1
2	USD	2.50	1
3	XTR	150.00	1
4	RUB	499.00	2
5	RUB	4990.00	3
6	RUB	0.00	4
7	USD	4.99	5
\.


--
-- Data for Name: plans; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.plans (id, order_index, is_active, type, availability, name, traffic_limit, device_limit, allowed_telegram_ids, internal_squads, created_at, updated_at, description, tag, traffic_limit_strategy, external_squad, is_trial, public_code, allowed_emails) FROM stdin;
1	1	t	BOTH	ALL	Базовый	100	3	{}	{11111111-1111-4111-8111-111111111111}	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02	Пример тарифа для теста	\N	MONTH	\N	f	pubA0001	{}
2	2	t	BOTH	NEW	Пробный	10	1	{}	{11111111-1111-4111-8111-111111111111}	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02	\N	\N	NO_RESET	\N	t	pubA0002	{}
3	3	t	UNLIMITED	ALLOWED	Безлимит	0	0	{2002}	{11111111-1111-4111-8111-111111111111,22222222-2222-4222-8222-222222222222}	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02	\N	\N	NO_RESET	33333333-3333-4333-8333-333333333333	f	pubA0003	{}
\.


--
-- Data for Name: promocode_activations; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.promocode_activations (id, promocode_id, user_id, activated_at) FROM stdin;
1	1	1	2026-03-03 12:00:00+02
2	2	2	2026-03-06 13:57:00+02
\.


--
-- Data for Name: promocodes; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.promocodes (id, code, is_active, reward_type, reward, plan_snapshot, availability, expires_at, max_activations, created_at, updated_at, is_reusable) FROM stdin;
1	WELCOME7	t	DURATION	7	\N	ALL	2027-01-01 01:59:00+02	100	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02	f
2	SALE20	t	PURCHASE_DISCOUNT	20	\N	NEW	\N	\N	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02	f
3	VIPFOREVER	f	PERSONAL_DISCOUNT	15	\N	EXISTING	\N	10	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02	t
4	GIGA10	t	TRAFFIC	10	\N	ALL	\N	\N	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02	f
5	PLAN90	t	SUBSCRIPTION	\N	{"id": 1, "name": "Базовый", "duration": 90}	ALL	\N	5	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02	f
6	ЛЕТО	t	DURATION	3	\N	ALL	\N	\N	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02	f
\.


--
-- Data for Name: referral_rewards; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.referral_rewards (id, referral_id, type, amount, is_issued, created_at, updated_at, user_id) FROM stdin;
\.


--
-- Data for Name: referrals; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.referrals (id, level, created_at, updated_at, referrer_id, referred_id) FROM stdin;
1	FIRST	2026-03-05 14:00:00+02	2026-03-05 14:00:00+02	1	2
\.


--
-- Data for Name: settings; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.settings (id, default_currency, referral, access, requirements, notifications, menu, created_at, updated_at, backup, blacklist, extra) FROM stdin;
1	RUB	{"level": 1, "enable": true, "reward": {"type": "EXTRA_DAYS", "config": {"1": 5}, "strategy": "AMOUNT"}, "accrual_strategy": "ON_FIRST_PAYMENT"}	{}	{}	{}	{}	2026-03-01 10:00:00+02	2026-03-01 10:00:00+02	{"enabled": true, "max_files": 7, "send_to_chat": true, "interval_hours": 24}	{}	{}
\.


--
-- Data for Name: subscriptions; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.subscriptions (id, user_remna_id, status, is_trial, traffic_limit, device_limit, internal_squads, expire_at, url, plan_snapshot, created_at, updated_at, external_squad, traffic_limit_strategy, tag, user_id, device_single_reset_at, device_all_reset_at, link_reset_at, disabled_by_channel_leave) FROM stdin;
1	aaaaaaaa-0000-4000-8000-000000000001	ACTIVE	f	100	4	{11111111-1111-4111-8111-111111111111}	2026-12-20 12:00:00+02	https://sub.example.com/pPetr7xQ	{"id": 1, "tag": null, "name": "Базовый", "type": "BOTH", "duration": 30, "is_trial": false, "device_limit": 3, "traffic_limit": 100, "external_squad": null, "internal_squads": ["11111111-1111-4111-8111-111111111111"], "traffic_limit_strategy": "MONTH"}	2026-03-02 12:05:00+02	2026-11-20 12:00:00+02	\N	MONTH	\N	1	\N	\N	\N	f
2	bbbbbbbb-0000-4000-8000-000000000002	DELETED	t	10	1	{11111111-1111-4111-8111-111111111111}	2026-03-08 14:00:00+02	https://sub.example.com/oLga0001	{"id": 2, "name": "Пробный", "type": "BOTH", "duration": 3, "is_trial": true, "device_limit": 1, "traffic_limit": 10, "external_squad": null, "internal_squads": [], "traffic_limit_strategy": "NO_RESET"}	2026-03-05 14:01:00+02	2026-03-06 14:00:00+02	\N	NO_RESET	\N	2	\N	\N	\N	f
3	bbbbbbbb-0000-4000-8000-000000000002	ACTIVE	f	0	0	{11111111-1111-4111-8111-111111111111,22222222-2222-4222-8222-222222222222}	2026-12-01 14:00:00+02	https://sub.example.com/oLga0001	{"id": 3, "name": "Безлимит", "type": "UNLIMITED", "duration": 30, "is_trial": false, "device_limit": 0, "traffic_limit": 0, "external_squad": "33333333-3333-4333-8333-333333333333", "internal_squads": [], "traffic_limit_strategy": "NO_RESET"}	2026-03-06 14:00:00+02	2026-11-01 14:00:00+02	33333333-3333-4333-8333-333333333333	NO_RESET	\N	2	\N	\N	\N	f
4	cccccccc-0000-4000-8000-000000000003	EXPIRED	f	100	3	{11111111-1111-4111-8111-111111111111}	2026-10-01 12:00:00+03	https://sub.example.com/mAria123	{"id": 1, "name": "Базовый", "type": "BOTH", "duration": 30, "is_trial": false, "device_limit": 3, "traffic_limit": 100, "external_squad": null, "internal_squads": [], "traffic_limit_strategy": "MONTH"}	2026-04-01 12:10:00+03	2026-10-01 12:00:00+03	\N	MONTH	\N	3	\N	\N	\N	f
\.


--
-- Data for Name: transactions; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.transactions (id, payment_id, status, is_test, purchase_type, gateway_type, pricing, currency, plan_snapshot, created_at, updated_at, user_id, gateway_display_name, payment_method) FROM stdin;
1	dddddddd-0000-4000-8000-000000000001	COMPLETED	f	NEW	YOOKASSA	{"final_amount": 199.0, "original_amount": 199.0, "discount_percent": 0}	RUB	{"id": 1, "name": "Базовый", "duration": 30}	2026-03-02 12:03:00+02	2026-03-02 12:05:00+02	1	\N	bank_card
2	dddddddd-0000-4000-8000-000000000002	COMPLETED	f	RENEW	TELEGRAM_STARS	{"final_amount": 150, "original_amount": 150, "discount_percent": 0}	XTR	{"id": 1, "name": "Базовый", "duration": 30}	2026-04-02 13:00:00+03	2026-04-02 13:01:00+03	1	\N	\N
3	dddddddd-0000-4000-8000-000000000003	COMPLETED	f	CHANGE	CRYPTOPAY	{"final_amount": 4.99, "original_amount": 4.99, "discount_percent": 0}	USD	{"id": 3, "name": "Безлимит", "duration": 30}	2026-03-06 13:58:00+02	2026-03-06 14:00:00+02	2	\N	\N
4	dddddddd-0000-4000-8000-000000000004	PENDING	f	RENEW	YOOKASSA	{"final_amount": 199.0, "original_amount": 199.0, "discount_percent": 0}	RUB	{"id": 3, "name": "Безлимит", "duration": 30}	2026-11-10 11:00:00+02	2026-11-10 11:00:00+02	2	\N	\N
5	dddddddd-0000-4000-8000-000000000005	REFUNDED	f	NEW	HELEKET	{"final_amount": 2.5, "original_amount": 2.5, "discount_percent": 0}	USD	{"id": 1, "name": "Базовый", "duration": 30}	2026-04-01 12:05:00+03	2026-04-03 12:00:00+03	3	\N	\N
6	dddddddd-0000-4000-8000-000000000006	COMPLETED	t	NEW	YOOKASSA	{"final_amount": 2, "original_amount": 2, "discount_percent": 0}	RUB	{"id": -1, "name": "test", "duration": 1}	2026-03-01 10:10:00+02	2026-03-01 10:10:00+02	1	\N	\N
7	dddddddd-0000-4000-8000-000000000007	COMPLETED	f	NEW	YOOKASSA	{"final_amount": 0, "original_amount": 199.0, "discount_percent": 100}	RUB	{"id": 1, "name": "Базовый", "duration": 30}	2026-04-01 12:08:00+03	2026-04-01 12:09:00+03	3	\N	\N
8	dddddddd-0000-4000-8000-000000000008	COMPLETED	f	RENEW	PLATEGA	{"final_amount": 499.0, "original_amount": 499.0, "discount_percent": 0}	RUB	{"id": 99, "name": "Старый тариф", "duration": 90}	2026-08-02 13:00:00+03	2026-08-02 13:02:00+03	1	\N	sbp
\.


--
-- Data for Name: user_oauth_providers; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.user_oauth_providers (id, user_id, provider, provider_id, created_at, updated_at) FROM stdin;
1	3	google	google-1093	2026-04-01 12:00:00+03	2026-04-01 12:00:00+03
2	3	yandex	yandex-77	2026-04-01 12:00:00+03	2026-04-01 12:00:00+03
\.


--
-- Data for Name: users; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.users (id, telegram_id, username, name, role, language, personal_discount, purchase_discount, is_blocked, is_bot_blocked, current_subscription_id, created_at, updated_at, referral_code, points, is_rules_accepted, is_trial_available, email, password_hash, is_email_verified, pending_email, email_verification_code_hash, email_verification_expires_at, auth_type, ad_link_id, referral_code_reset_at) FROM stdin;
4	2004	ivan_ad	Иван	ADMIN	RU	0	0	t	f	\N	2026-04-10 12:00:00+03	2026-04-10 12:00:00+03	Iv44Ad	0	t	t	\N	\N	f	\N	\N	\N	telegram	1	\N
1	2001	petr_k	Пётр	USER	RU	0	0	f	f	1	2026-03-02 12:00:00+02	2026-03-02 12:00:00+02	aB3dE9	120	t	f	\N	\N	f	\N	\N	\N	telegram	\N	\N
2	2002	\N	Olga	USER	EN	10	0	f	t	3	2026-03-05 14:00:00+02	2026-03-05 14:00:00+02	Zx81Qp	0	t	f	\N	\N	f	\N	\N	\N	telegram	\N	\N
3	\N	\N	Мария	USER	RU	0	0	f	f	4	2026-04-01 12:00:00+03	2026-04-01 12:00:00+03	Mm77Kk	0	t	f	Maria@Example.com	scrypt$16384$8$1$Zml4dHVyZS1zYWx0$Zml4dHVyZS1kaWdlc3Q	t	\N	\N	\N	email	\N	\N
\.


--
-- Name: ad_links_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.ad_links_id_seq', 100, true);


--
-- Name: broadcast_messages_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.broadcast_messages_id_seq', 1, false);


--
-- Name: broadcasts_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.broadcasts_id_seq', 1, false);


--
-- Name: payment_gateways_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.payment_gateways_id_seq', 100, true);


--
-- Name: plan_durations_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.plan_durations_id_seq', 100, true);


--
-- Name: plan_prices_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.plan_prices_id_seq', 100, true);


--
-- Name: plans_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.plans_id_seq', 100, true);


--
-- Name: promocode_activations_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.promocode_activations_id_seq', 100, true);


--
-- Name: promocodes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.promocodes_id_seq', 100, true);


--
-- Name: referral_rewards_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.referral_rewards_id_seq', 1, false);


--
-- Name: referrals_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.referrals_id_seq', 100, true);


--
-- Name: settings_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.settings_id_seq', 100, true);


--
-- Name: subscriptions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.subscriptions_id_seq', 100, true);


--
-- Name: transactions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.transactions_id_seq', 100, true);


--
-- Name: user_oauth_providers_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.user_oauth_providers_id_seq', 100, true);


--
-- Name: users_id_seq; Type: SEQUENCE SET; Schema: public; Owner: postgres
--

SELECT pg_catalog.setval('public.users_id_seq', 100, true);


--
-- Name: ad_links ad_links_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.ad_links
    ADD CONSTRAINT ad_links_pkey PRIMARY KEY (id);


--
-- Name: alembic_version alembic_version_pkc; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.alembic_version
    ADD CONSTRAINT alembic_version_pkc PRIMARY KEY (version_num);


--
-- Name: broadcast_messages broadcast_messages_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.broadcast_messages
    ADD CONSTRAINT broadcast_messages_pkey PRIMARY KEY (id);


--
-- Name: broadcasts broadcasts_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.broadcasts
    ADD CONSTRAINT broadcasts_pkey PRIMARY KEY (id);


--
-- Name: broadcasts broadcasts_task_id_key; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.broadcasts
    ADD CONSTRAINT broadcasts_task_id_key UNIQUE (task_id);


--
-- Name: payment_gateways payment_gateways_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.payment_gateways
    ADD CONSTRAINT payment_gateways_pkey PRIMARY KEY (id);


--
-- Name: payment_gateways payment_gateways_type_key; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.payment_gateways
    ADD CONSTRAINT payment_gateways_type_key UNIQUE (type);


--
-- Name: plan_durations plan_durations_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.plan_durations
    ADD CONSTRAINT plan_durations_pkey PRIMARY KEY (id);


--
-- Name: plan_prices plan_prices_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.plan_prices
    ADD CONSTRAINT plan_prices_pkey PRIMARY KEY (id);


--
-- Name: plans plans_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.plans
    ADD CONSTRAINT plans_pkey PRIMARY KEY (id);


--
-- Name: promocode_activations promocode_activations_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.promocode_activations
    ADD CONSTRAINT promocode_activations_pkey PRIMARY KEY (id);


--
-- Name: promocodes promocodes_code_key; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.promocodes
    ADD CONSTRAINT promocodes_code_key UNIQUE (code);


--
-- Name: promocodes promocodes_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.promocodes
    ADD CONSTRAINT promocodes_pkey PRIMARY KEY (id);


--
-- Name: referral_rewards referral_rewards_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.referral_rewards
    ADD CONSTRAINT referral_rewards_pkey PRIMARY KEY (id);


--
-- Name: referrals referrals_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.referrals
    ADD CONSTRAINT referrals_pkey PRIMARY KEY (id);


--
-- Name: settings settings_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.settings
    ADD CONSTRAINT settings_pkey PRIMARY KEY (id);


--
-- Name: subscriptions subscriptions_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.subscriptions
    ADD CONSTRAINT subscriptions_pkey PRIMARY KEY (id);


--
-- Name: transactions transactions_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.transactions
    ADD CONSTRAINT transactions_pkey PRIMARY KEY (id);


--
-- Name: ad_links uq_ad_links_code; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.ad_links
    ADD CONSTRAINT uq_ad_links_code UNIQUE (code);


--
-- Name: user_oauth_providers uq_user_oauth_providers_provider_id; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.user_oauth_providers
    ADD CONSTRAINT uq_user_oauth_providers_provider_id UNIQUE (provider, provider_id);


--
-- Name: user_oauth_providers uq_user_oauth_providers_user_provider; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.user_oauth_providers
    ADD CONSTRAINT uq_user_oauth_providers_user_provider UNIQUE (user_id, provider);


--
-- Name: user_oauth_providers user_oauth_providers_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.user_oauth_providers
    ADD CONSTRAINT user_oauth_providers_pkey PRIMARY KEY (id);


--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);


--
-- Name: ix_ad_links_code; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_ad_links_code ON public.ad_links USING btree (code);


--
-- Name: ix_ad_links_name; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_ad_links_name ON public.ad_links USING btree (name);


--
-- Name: ix_broadcast_messages_broadcast_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_broadcast_messages_broadcast_id ON public.broadcast_messages USING btree (broadcast_id);


--
-- Name: ix_broadcast_messages_status; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_broadcast_messages_status ON public.broadcast_messages USING btree (status);


--
-- Name: ix_broadcast_messages_user_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_broadcast_messages_user_id ON public.broadcast_messages USING btree (user_id);


--
-- Name: ix_broadcasts_status; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_broadcasts_status ON public.broadcasts USING btree (status);


--
-- Name: ix_payment_gateways_order_index; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_payment_gateways_order_index ON public.payment_gateways USING btree (order_index);


--
-- Name: ix_plan_durations_order_index; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_plan_durations_order_index ON public.plan_durations USING btree (order_index);


--
-- Name: ix_plan_durations_plan_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_plan_durations_plan_id ON public.plan_durations USING btree (plan_id);


--
-- Name: ix_plan_prices_plan_duration_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_plan_prices_plan_duration_id ON public.plan_prices USING btree (plan_duration_id);


--
-- Name: ix_plans_name; Type: INDEX; Schema: public; Owner: postgres
--

CREATE UNIQUE INDEX ix_plans_name ON public.plans USING btree (name);


--
-- Name: ix_plans_order_index; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_plans_order_index ON public.plans USING btree (order_index);


--
-- Name: ix_plans_public_code; Type: INDEX; Schema: public; Owner: postgres
--

CREATE UNIQUE INDEX ix_plans_public_code ON public.plans USING btree (public_code);


--
-- Name: ix_promocode_activations_promocode_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_promocode_activations_promocode_id ON public.promocode_activations USING btree (promocode_id);


--
-- Name: ix_promocode_activations_user_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_promocode_activations_user_id ON public.promocode_activations USING btree (user_id);


--
-- Name: ix_promocodes_code; Type: INDEX; Schema: public; Owner: postgres
--

CREATE UNIQUE INDEX ix_promocodes_code ON public.promocodes USING btree (code);


--
-- Name: ix_referral_rewards_referral_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_referral_rewards_referral_id ON public.referral_rewards USING btree (referral_id);


--
-- Name: ix_referral_rewards_user_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_referral_rewards_user_id ON public.referral_rewards USING btree (user_id);


--
-- Name: ix_referrals_referred_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE UNIQUE INDEX ix_referrals_referred_id ON public.referrals USING btree (referred_id);


--
-- Name: ix_referrals_referrer_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_referrals_referrer_id ON public.referrals USING btree (referrer_id);


--
-- Name: ix_subscriptions_expire_at; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_subscriptions_expire_at ON public.subscriptions USING btree (expire_at);


--
-- Name: ix_subscriptions_status; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_subscriptions_status ON public.subscriptions USING btree (status);


--
-- Name: ix_subscriptions_user_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_subscriptions_user_id ON public.subscriptions USING btree (user_id);


--
-- Name: ix_subscriptions_user_remna_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_subscriptions_user_remna_id ON public.subscriptions USING btree (user_remna_id);


--
-- Name: ix_transactions_payment_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE UNIQUE INDEX ix_transactions_payment_id ON public.transactions USING btree (payment_id);


--
-- Name: ix_transactions_status; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_transactions_status ON public.transactions USING btree (status);


--
-- Name: ix_transactions_user_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_transactions_user_id ON public.transactions USING btree (user_id);


--
-- Name: ix_user_oauth_providers_user_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_user_oauth_providers_user_id ON public.user_oauth_providers USING btree (user_id);


--
-- Name: ix_users_ad_link_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_users_ad_link_id ON public.users USING btree (ad_link_id);


--
-- Name: ix_users_current_subscription_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_users_current_subscription_id ON public.users USING btree (current_subscription_id);


--
-- Name: ix_users_email; Type: INDEX; Schema: public; Owner: postgres
--

CREATE UNIQUE INDEX ix_users_email ON public.users USING btree (email);


--
-- Name: ix_users_referral_code; Type: INDEX; Schema: public; Owner: postgres
--

CREATE UNIQUE INDEX ix_users_referral_code ON public.users USING btree (referral_code);


--
-- Name: ix_users_role; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_users_role ON public.users USING btree (role);


--
-- Name: ix_users_telegram_id; Type: INDEX; Schema: public; Owner: postgres
--

CREATE UNIQUE INDEX ix_users_telegram_id ON public.users USING btree (telegram_id);


--
-- Name: ix_users_username; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX ix_users_username ON public.users USING btree (username);


--
-- Name: broadcast_messages broadcast_messages_broadcast_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.broadcast_messages
    ADD CONSTRAINT broadcast_messages_broadcast_id_fkey FOREIGN KEY (broadcast_id) REFERENCES public.broadcasts(id) ON DELETE CASCADE;


--
-- Name: broadcast_messages broadcast_messages_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.broadcast_messages
    ADD CONSTRAINT broadcast_messages_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: users fk_users_ad_link_id; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT fk_users_ad_link_id FOREIGN KEY (ad_link_id) REFERENCES public.ad_links(id) ON DELETE SET NULL;


--
-- Name: users fk_users_current_subscription_id; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT fk_users_current_subscription_id FOREIGN KEY (current_subscription_id) REFERENCES public.subscriptions(id) ON DELETE SET NULL;


--
-- Name: plan_durations plan_durations_plan_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.plan_durations
    ADD CONSTRAINT plan_durations_plan_id_fkey FOREIGN KEY (plan_id) REFERENCES public.plans(id) ON DELETE CASCADE;


--
-- Name: plan_prices plan_prices_plan_duration_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.plan_prices
    ADD CONSTRAINT plan_prices_plan_duration_id_fkey FOREIGN KEY (plan_duration_id) REFERENCES public.plan_durations(id) ON DELETE CASCADE;


--
-- Name: promocode_activations promocode_activations_promocode_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.promocode_activations
    ADD CONSTRAINT promocode_activations_promocode_id_fkey FOREIGN KEY (promocode_id) REFERENCES public.promocodes(id) ON DELETE CASCADE;


--
-- Name: promocode_activations promocode_activations_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.promocode_activations
    ADD CONSTRAINT promocode_activations_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: referral_rewards referral_rewards_referral_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.referral_rewards
    ADD CONSTRAINT referral_rewards_referral_id_fkey FOREIGN KEY (referral_id) REFERENCES public.referrals(id) ON DELETE CASCADE;


--
-- Name: referral_rewards referral_rewards_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.referral_rewards
    ADD CONSTRAINT referral_rewards_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: referrals referrals_referred_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.referrals
    ADD CONSTRAINT referrals_referred_id_fkey FOREIGN KEY (referred_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: referrals referrals_referrer_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.referrals
    ADD CONSTRAINT referrals_referrer_id_fkey FOREIGN KEY (referrer_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: subscriptions subscriptions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.subscriptions
    ADD CONSTRAINT subscriptions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: transactions transactions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.transactions
    ADD CONSTRAINT transactions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: user_oauth_providers user_oauth_providers_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.user_oauth_providers
    ADD CONSTRAINT user_oauth_providers_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- PostgreSQL database dump complete
--

\unrestrict 1xjNygS13o1XvuENssevIhLS1KMmASs8cAJjtGQC8aaAdZuaChQ0PrTvqip7jyg

