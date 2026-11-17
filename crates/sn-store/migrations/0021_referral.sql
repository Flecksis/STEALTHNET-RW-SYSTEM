CREATE TABLE referral_codes (
    client_id bigint PRIMARY KEY REFERENCES clients (id) ON DELETE CASCADE,
    code text NOT NULL CHECK (code ~ '^[A-Z0-9]{4,32}$'),
    percent integer CHECK (percent BETWEEN 0 AND 100),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT referral_codes_code_key UNIQUE (code)
);

CREATE TABLE referral_links (
    client_id bigint PRIMARY KEY REFERENCES clients (id) ON DELETE CASCADE,
    referrer_id bigint NOT NULL REFERENCES clients (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT referral_links_self_check CHECK (client_id <> referrer_id)
);
CREATE INDEX referral_links_referrer_idx ON referral_links (referrer_id);

CREATE TABLE referral_payments (
    payment_id bigint PRIMARY KEY REFERENCES payments (id),
    processed_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE referral_credits (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    payment_id bigint NOT NULL REFERENCES payments (id),
    level integer NOT NULL CHECK (level BETWEEN 1 AND 3),
    referrer_id bigint NOT NULL REFERENCES clients (id),
    referral_id bigint NOT NULL REFERENCES clients (id),
    base_minor bigint NOT NULL CHECK (base_minor > 0),
    percent integer NOT NULL CHECK (percent BETWEEN 1 AND 100),
    amount_minor bigint NOT NULL CHECK (amount_minor > 0),
    currency text NOT NULL CHECK (currency IN ('USD', 'RUB')),
    credited_at timestamptz,
    reversal_target_minor bigint NOT NULL DEFAULT 0 CHECK (reversal_target_minor >= 0),
    reversed_minor bigint NOT NULL DEFAULT 0 CHECK (reversed_minor >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT referral_credits_payment_level_key UNIQUE (payment_id, level),
    CONSTRAINT referral_credits_reversal_check CHECK (reversal_target_minor <= amount_minor AND reversed_minor <= reversal_target_minor)
);
CREATE INDEX referral_credits_referrer_idx ON referral_credits (referrer_id, id DESC);
CREATE INDEX referral_credits_unsettled_idx ON referral_credits (id) WHERE credited_at IS NULL OR reversed_minor < reversal_target_minor;
CREATE TRIGGER referral_credits_touch BEFORE UPDATE ON referral_credits FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE referral_withdrawals (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id uuid NOT NULL DEFAULT gen_random_uuid(),
    client_id bigint NOT NULL REFERENCES clients (id),
    amount_minor bigint NOT NULL CHECK (amount_minor > 0),
    currency text NOT NULL CHECK (currency IN ('USD', 'RUB')),
    method text NOT NULL CHECK (method IN ('sbp', 'card', 'usdt_trc20')),
    details text NOT NULL CHECK (btrim(details) <> ''),
    status text NOT NULL DEFAULT 'holding' CHECK (status IN ('holding', 'pending', 'paid', 'rejected', 'canceled')),
    admin_id bigint REFERENCES admins (id),
    comment text,
    held_at timestamptz,
    released_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    decided_at timestamptz,
    CONSTRAINT referral_withdrawals_public_id_key UNIQUE (public_id),
    CONSTRAINT referral_withdrawals_release_check CHECK (released_at IS NULL OR held_at IS NOT NULL)
);
CREATE INDEX referral_withdrawals_status_idx ON referral_withdrawals (status, id DESC);
CREATE INDEX referral_withdrawals_client_idx ON referral_withdrawals (client_id, id DESC);
CREATE TRIGGER referral_withdrawals_touch BEFORE UPDATE ON referral_withdrawals FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
