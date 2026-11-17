CREATE FUNCTION touch_updated_at() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END
$$;

CREATE TABLE settings (
    key text PRIMARY KEY,
    value jsonb NOT NULL,
    updated_by bigint,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE secrets (
    key text PRIMARY KEY,
    nonce bytea NOT NULL,
    ciphertext bytea NOT NULL,
    updated_by bigint,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE admins (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email text NOT NULL,
    password_hash text NOT NULL,
    role text NOT NULL CHECK (role IN ('owner', 'admin', 'support', 'readonly')),
    permissions text[] NOT NULL DEFAULT '{}',
    totp_secret bytea,
    totp_nonce bytea,
    totp_last_step bigint,
    telegram_id bigint,
    must_change_password boolean NOT NULL DEFAULT false,
    disabled_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX admins_email_key ON admins (lower(email));
CREATE TRIGGER admins_touch BEFORE UPDATE ON admins FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE admin_sessions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    admin_id bigint NOT NULL REFERENCES admins (id) ON DELETE CASCADE,
    token_hash bytea NOT NULL,
    ip text,
    user_agent text,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    CONSTRAINT admin_sessions_token_key UNIQUE (token_hash)
);
CREATE INDEX admin_sessions_admin_idx ON admin_sessions (admin_id);

CREATE TABLE clients (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id uuid NOT NULL DEFAULT gen_random_uuid(),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'blocked')),
    block_reason text,
    blocked_at timestamptz,
    locale text NOT NULL DEFAULT 'ru' CHECK (locale IN ('ru', 'en')),
    note text,
    registration_source text NOT NULL CHECK (registration_source IN ('bot', 'miniapp', 'web', 'admin', 'import', 'migration')),
    bot_unreachable boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT clients_public_id_key UNIQUE (public_id)
);
CREATE INDEX clients_created_idx ON clients (id DESC) WHERE deleted_at IS NULL;
CREATE TRIGGER clients_touch BEFORE UPDATE ON clients FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE client_identities (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('telegram', 'email', 'google', 'apple')),
    value text NOT NULL,
    verified boolean NOT NULL DEFAULT false,
    meta jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT client_identities_kind_value_key UNIQUE (kind, value)
);
CREATE INDEX client_identities_client_idx ON client_identities (client_id);
CREATE TRIGGER client_identities_touch BEFORE UPDATE ON client_identities FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE client_credentials (
    client_id bigint PRIMARY KEY REFERENCES clients (id) ON DELETE CASCADE,
    password_hash text,
    totp_secret bytea,
    totp_nonce bytea,
    totp_last_step bigint,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TRIGGER client_credentials_touch BEFORE UPDATE ON client_credentials FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE client_sessions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    token_hash bytea NOT NULL,
    channel text NOT NULL CHECK (channel IN ('cabinet', 'miniapp')),
    scope text NOT NULL DEFAULT 'full' CHECK (scope IN ('full', 'subscription_view')),
    ip text,
    user_agent text,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    CONSTRAINT client_sessions_token_key UNIQUE (token_hash)
);
CREATE INDEX client_sessions_client_idx ON client_sessions (client_id);

CREATE TABLE trial_claims (
    kind text NOT NULL CHECK (kind IN ('telegram', 'email', 'google', 'apple')),
    value text NOT NULL,
    client_id bigint REFERENCES clients (id) ON DELETE SET NULL,
    claimed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, value)
);

CREATE TABLE tariff_categories (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL CHECK (btrim(name) <> ''),
    sort integer NOT NULL DEFAULT 0,
    visible boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz
);
CREATE TRIGGER tariff_categories_touch BEFORE UPDATE ON tariff_categories FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE tariffs (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    category_id bigint NOT NULL REFERENCES tariff_categories (id),
    name text NOT NULL CHECK (btrim(name) <> ''),
    description text,
    on_sale boolean NOT NULL DEFAULT true,
    sort integer NOT NULL DEFAULT 0,
    devices_included integer NOT NULL DEFAULT 1 CHECK (devices_included >= 0),
    extra_devices_enabled boolean NOT NULL DEFAULT false,
    extra_devices_max integer NOT NULL DEFAULT 0 CHECK (extra_devices_max >= 0),
    extra_device_usd_minor bigint CHECK (extra_device_usd_minor >= 0),
    extra_device_rub_minor bigint CHECK (extra_device_rub_minor >= 0),
    extra_device_stars bigint CHECK (extra_device_stars > 0),
    traffic_limit_bytes bigint CHECK (traffic_limit_bytes > 0),
    traffic_reset text NOT NULL DEFAULT 'no_reset' CHECK (traffic_reset IN ('no_reset', 'day', 'week', 'month', 'month_rolling', 'on_payment')),
    external_squad_uuid uuid,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz
);
CREATE INDEX tariffs_category_idx ON tariffs (category_id);
CREATE TRIGGER tariffs_touch BEFORE UPDATE ON tariffs FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE tariff_periods (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tariff_id bigint NOT NULL REFERENCES tariffs (id) ON DELETE CASCADE,
    days integer NOT NULL CHECK (days > 0),
    price_usd_minor bigint CHECK (price_usd_minor >= 0),
    price_rub_minor bigint CHECK (price_rub_minor >= 0),
    stars bigint CHECK (stars > 0),
    sort integer NOT NULL DEFAULT 0,
    active boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT tariff_periods_tariff_days_key UNIQUE (tariff_id, days)
);
CREATE TRIGGER tariff_periods_touch BEFORE UPDATE ON tariff_periods FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE tariff_squads (
    tariff_id bigint NOT NULL REFERENCES tariffs (id) ON DELETE CASCADE,
    squad_uuid uuid NOT NULL,
    position integer NOT NULL DEFAULT 0,
    PRIMARY KEY (tariff_id, squad_uuid)
);

CREATE TABLE subscriptions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id uuid NOT NULL DEFAULT gen_random_uuid(),
    client_id bigint NOT NULL REFERENCES clients (id),
    tariff_id bigint REFERENCES tariffs (id) ON DELETE SET NULL,
    kind text NOT NULL CHECK (kind IN ('paid', 'trial', 'gift', 'promo', 'manual', 'custom')),
    remna_user_id bigint,
    remna_username text,
    short_uuid text,
    extra_devices integer NOT NULL DEFAULT 0 CHECK (extra_devices >= 0),
    period_started_at timestamptz,
    period_days integer CHECK (period_days > 0),
    autorenew_enabled boolean NOT NULL DEFAULT false,
    autorenew_period_id bigint REFERENCES tariff_periods (id) ON DELETE SET NULL,
    status text CHECK (status IN ('active', 'limited', 'expired', 'disabled')),
    expire_at timestamptz,
    traffic_used_bytes bigint,
    traffic_limit_bytes bigint,
    traffic_strategy text CHECK (traffic_strategy IN ('NO_RESET', 'DAY', 'WEEK', 'MONTH', 'MONTH_ROLLING')),
    hwid_limit integer,
    devices_count integer,
    online_at timestamptz,
    first_connected_at timestamptz,
    last_node_uuid uuid,
    remna_updated_at timestamptz,
    synced_at timestamptz,
    remna_missing_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    ended_at timestamptz,
    CONSTRAINT subscriptions_public_id_key UNIQUE (public_id),
    CONSTRAINT subscriptions_remna_user_key UNIQUE (remna_user_id),
    CONSTRAINT subscriptions_remna_username_key UNIQUE (remna_username)
);
CREATE INDEX subscriptions_client_idx ON subscriptions (client_id);
CREATE INDEX subscriptions_expire_idx ON subscriptions (expire_at) WHERE ended_at IS NULL;
CREATE TRIGGER subscriptions_touch BEFORE UPDATE ON subscriptions FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE orders (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id uuid NOT NULL DEFAULT gen_random_uuid(),
    client_id bigint NOT NULL REFERENCES clients (id),
    subscription_id bigint REFERENCES subscriptions (id),
    kind text NOT NULL CHECK (kind IN ('new', 'renewal', 'change', 'topup', 'trial', 'module', 'admin_grant')),
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'paid', 'fulfilled', 'review', 'canceled', 'refunded')),
    intent jsonb NOT NULL,
    quote jsonb NOT NULL,
    snapshot jsonb,
    plan jsonb,
    currency text NOT NULL CHECK (currency IN ('USD', 'RUB')),
    total_minor bigint NOT NULL CHECK (total_minor >= 0),
    stars_total bigint CHECK (stars_total >= 0),
    review_reason text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    paid_at timestamptz,
    fulfilled_at timestamptz,
    canceled_at timestamptz,
    CONSTRAINT orders_public_id_key UNIQUE (public_id)
);
CREATE INDEX orders_client_idx ON orders (client_id, id DESC);
CREATE INDEX orders_attention_idx ON orders (status, updated_at) WHERE status IN ('paid', 'review');
CREATE TRIGGER orders_touch BEFORE UPDATE ON orders FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE payments (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id uuid NOT NULL DEFAULT gen_random_uuid(),
    order_id bigint NOT NULL REFERENCES orders (id),
    client_id bigint NOT NULL REFERENCES clients (id),
    provider text NOT NULL,
    method text,
    amount_minor bigint NOT NULL CHECK (amount_minor >= 0),
    currency text NOT NULL CHECK (currency IN ('USD', 'RUB', 'XTR')),
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'paid', 'failed', 'canceled', 'refunded', 'partially_refunded')),
    provider_txid text,
    pay_url text,
    provider_payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    failure_reason text,
    refunded_minor bigint NOT NULL DEFAULT 0 CHECK (refunded_minor >= 0),
    expires_at timestamptz,
    paid_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT payments_public_id_key UNIQUE (public_id),
    CONSTRAINT payments_provider_txid_key UNIQUE (provider, provider_txid)
);
CREATE INDEX payments_order_idx ON payments (order_id);
CREATE INDEX payments_client_idx ON payments (client_id, id DESC);
CREATE INDEX payments_pending_idx ON payments (expires_at) WHERE status = 'pending';
CREATE TRIGGER payments_touch BEFORE UPDATE ON payments FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE refunds (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    payment_id bigint NOT NULL REFERENCES payments (id),
    amount_minor bigint NOT NULL CHECK (amount_minor > 0),
    currency text NOT NULL CHECK (currency IN ('USD', 'RUB', 'XTR')),
    method text NOT NULL CHECK (method IN ('balance', 'provider')),
    subscription_action text NOT NULL CHECK (subscription_action IN ('keep', 'cut', 'disable')),
    reason text,
    admin_id bigint REFERENCES admins (id),
    provider_refund_id text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX refunds_payment_idx ON refunds (payment_id);

CREATE TABLE saved_payment_methods (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    provider text NOT NULL,
    token_nonce bytea NOT NULL,
    token_ciphertext bytea NOT NULL,
    title text,
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);
CREATE UNIQUE INDEX saved_payment_methods_active_key ON saved_payment_methods (client_id, provider) WHERE revoked_at IS NULL;

CREATE TABLE balance_entries (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id bigint NOT NULL REFERENCES clients (id),
    wallet text NOT NULL CHECK (wallet IN ('main', 'referral')),
    currency text NOT NULL CHECK (currency IN ('USD', 'RUB')),
    delta_minor bigint NOT NULL CHECK (delta_minor <> 0),
    balance_after_minor bigint NOT NULL CHECK (balance_after_minor >= 0),
    kind text NOT NULL CHECK (kind IN ('topup', 'purchase', 'autorenew', 'refund', 'referral_credit', 'referral_reversal', 'withdrawal_hold', 'withdrawal_release', 'contest', 'roulette', 'admin_adjust', 'migration_opening')),
    order_id bigint REFERENCES orders (id),
    payment_id bigint REFERENCES payments (id),
    admin_id bigint REFERENCES admins (id),
    comment text,
    idempotency_key text,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT balance_entries_idempotency_key UNIQUE (idempotency_key)
);
CREATE INDEX balance_entries_client_idx ON balance_entries (client_id, id DESC);

CREATE TABLE client_balances (
    client_id bigint NOT NULL REFERENCES clients (id),
    wallet text NOT NULL CHECK (wallet IN ('main', 'referral')),
    currency text NOT NULL CHECK (currency IN ('USD', 'RUB')),
    amount_minor bigint NOT NULL DEFAULT 0 CHECK (amount_minor >= 0),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, wallet, currency)
);

CREATE TABLE autorenew_attempts (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    subscription_id bigint NOT NULL REFERENCES subscriptions (id),
    period_key text NOT NULL,
    attempt_no integer NOT NULL CHECK (attempt_no > 0),
    result text NOT NULL CHECK (result IN ('charged', 'insufficient', 'failed', 'skipped', 'reminded')),
    order_id bigint REFERENCES orders (id),
    details text,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT autorenew_attempts_key UNIQUE (subscription_id, period_key, attempt_no)
);

CREATE TABLE notifications_sent (
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    kind text NOT NULL,
    cycle_key text NOT NULL,
    sent_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, kind, cycle_key)
);

CREATE TABLE module_states (
    module text PRIMARY KEY,
    enabled boolean NOT NULL,
    config jsonb NOT NULL DEFAULT '{}'::jsonb,
    updated_by bigint REFERENCES admins (id),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE jobs (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind text NOT NULL,
    dedupe_key text,
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'done', 'failed', 'review')),
    run_at timestamptz NOT NULL DEFAULT now(),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts integer NOT NULL DEFAULT 10 CHECK (max_attempts > 0),
    locked_until timestamptz,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    done_at timestamptz,
    CONSTRAINT jobs_dedupe_key UNIQUE (dedupe_key)
);
CREATE INDEX jobs_ready_idx ON jobs (run_at) WHERE status IN ('queued', 'running');
CREATE INDEX jobs_attention_idx ON jobs (updated_at DESC) WHERE status IN ('failed', 'review');
CREATE TRIGGER jobs_touch BEFORE UPDATE ON jobs FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE domain_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind text NOT NULL,
    client_id bigint,
    payload jsonb NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    dispatched_at timestamptz
);
CREATE INDEX domain_events_pending_idx ON domain_events (id) WHERE dispatched_at IS NULL;
CREATE INDEX domain_events_client_idx ON domain_events (client_id, id DESC);

CREATE TABLE webhook_inbox (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source text NOT NULL,
    dedupe_key text NOT NULL,
    remote_ip text,
    headers jsonb NOT NULL DEFAULT '{}'::jsonb,
    body bytea,
    outcome text NOT NULL DEFAULT 'received' CHECK (outcome IN ('received', 'accepted', 'ignored', 'rejected_signature', 'rejected_payload', 'payment_not_found', 'amount_mismatch', 'error')),
    payment_id bigint REFERENCES payments (id),
    error text,
    received_at timestamptz NOT NULL DEFAULT now(),
    processed_at timestamptz,
    CONSTRAINT webhook_inbox_source_dedupe_key UNIQUE (source, dedupe_key)
);
CREATE INDEX webhook_inbox_received_idx ON webhook_inbox (received_at DESC);

CREATE TABLE remna_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    body_sha256 bytea NOT NULL,
    scope text NOT NULL,
    event text NOT NULL,
    occurred_at timestamptz NOT NULL,
    subject_kind text,
    subject_id text,
    payload jsonb NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    processed_at timestamptz,
    CONSTRAINT remna_events_body_key UNIQUE (body_sha256)
);
CREATE INDEX remna_events_subject_idx ON remna_events (subject_kind, subject_id, occurred_at DESC);
CREATE INDEX remna_events_feed_idx ON remna_events (occurred_at DESC);

CREATE TABLE audit_log (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_kind text NOT NULL CHECK (actor_kind IN ('admin', 'client', 'system', 'bot')),
    actor_id bigint,
    action text NOT NULL,
    entity_kind text,
    entity_id bigint,
    details jsonb NOT NULL DEFAULT '{}'::jsonb,
    ip text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_entity_idx ON audit_log (entity_kind, entity_id, id DESC);

CREATE TABLE tickets (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id bigint NOT NULL REFERENCES clients (id),
    subscription_id bigint REFERENCES subscriptions (id),
    channel text NOT NULL CHECK (channel IN ('bot', 'cabinet', 'miniapp', 'unknown')),
    status text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed')),
    subject text,
    last_author text NOT NULL DEFAULT 'client' CHECK (last_author IN ('client', 'support', 'ai', 'system')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    closed_at timestamptz
);
CREATE INDEX tickets_status_idx ON tickets (status, updated_at DESC);
CREATE INDEX tickets_client_idx ON tickets (client_id);
CREATE TRIGGER tickets_touch BEFORE UPDATE ON tickets FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE ticket_messages (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ticket_id bigint NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    author text NOT NULL CHECK (author IN ('client', 'support', 'ai', 'system')),
    admin_id bigint REFERENCES admins (id),
    body text NOT NULL,
    attachments jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    read_at timestamptz
);
CREATE INDEX ticket_messages_ticket_idx ON ticket_messages (ticket_id, id);

CREATE TABLE bot_state (
    key text PRIMARY KEY,
    value jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE bot_dialogs (
    chat_id bigint PRIMARY KEY,
    state jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE legacy_ids (
    entity text NOT NULL,
    old_id text NOT NULL,
    new_id bigint NOT NULL,
    PRIMARY KEY (entity, old_id)
);
