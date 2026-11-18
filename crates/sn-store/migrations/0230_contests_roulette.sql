CREATE TABLE contests (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    title text NOT NULL CHECK (btrim(title) <> '' AND char_length(title) <= 100),
    description text CHECK (description IS NULL OR char_length(description) <= 1000),
    prize jsonb NOT NULL CHECK (jsonb_typeof(prize) = 'object'),
    winners integer NOT NULL CHECK (winners BETWEEN 1 AND 100),
    conditions jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(conditions) = 'object'),
    starts_at timestamptz NOT NULL,
    ends_at timestamptz NOT NULL,
    seed bytea NOT NULL CHECK (octet_length(seed) = 32),
    seed_hash text NOT NULL,
    list_hash text,
    entries_drawn integer,
    drawn_at timestamptz,
    canceled_at timestamptz,
    created_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT contests_dates_check CHECK (ends_at > starts_at)
);
CREATE INDEX contests_list_idx ON contests (id DESC);
CREATE INDEX contests_due_idx ON contests (ends_at) WHERE drawn_at IS NULL AND canceled_at IS NULL;
CREATE TRIGGER contests_touch BEFORE UPDATE ON contests FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE contest_entries (
    contest_id bigint NOT NULL REFERENCES contests (id) ON DELETE CASCADE,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    joined_at timestamptz NOT NULL DEFAULT now(),
    excluded text,
    ticket text,
    place integer CHECK (place > 0),
    prize_state text CHECK (prize_state IN ('pending', 'waiting', 'issuing', 'issued', 'failed')),
    prize_note text,
    prize_code text,
    prize_error text,
    prize_at timestamptz,
    notified text CHECK (notified IN ('waiting', 'issued')),
    PRIMARY KEY (contest_id, client_id),
    CONSTRAINT contest_entries_winner_check CHECK ((place IS NULL) = (prize_state IS NULL))
);
CREATE INDEX contest_entries_client_idx ON contest_entries (client_id);
CREATE UNIQUE INDEX contest_entries_place_key ON contest_entries (contest_id, place) WHERE place IS NOT NULL;
CREATE INDEX contest_entries_prize_idx ON contest_entries (contest_id) WHERE prize_state IN ('pending', 'waiting');

CREATE TABLE contest_messages (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    contest_id bigint NOT NULL REFERENCES contests (id) ON DELETE CASCADE,
    audience text NOT NULL CHECK (audience IN ('outsiders', 'participants', 'results')),
    only_eligible boolean NOT NULL DEFAULT false,
    send_at timestamptz,
    text text NOT NULL CHECK (btrim(text) <> ''),
    queued_at timestamptz,
    recipients integer CHECK (recipients >= 0),
    skipped text,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT contest_messages_time_check CHECK ((audience = 'results') = (send_at IS NULL))
);
CREATE INDEX contest_messages_contest_idx ON contest_messages (contest_id, id);
CREATE INDEX contest_messages_due_idx ON contest_messages (send_at) WHERE queued_at IS NULL;

CREATE TABLE contest_log (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    contest_id bigint NOT NULL REFERENCES contests (id) ON DELETE CASCADE,
    kind text NOT NULL,
    text text NOT NULL,
    details jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX contest_log_contest_idx ON contest_log (contest_id, id DESC);

ALTER TABLE broadcasts ADD COLUMN contest_message_id bigint REFERENCES contest_messages (id);
CREATE UNIQUE INDEX broadcasts_contest_message_key ON broadcasts (contest_message_id) WHERE contest_message_id IS NOT NULL;
DROP INDEX broadcasts_once_idx;
CREATE INDEX broadcasts_once_idx ON broadcasts (id DESC) WHERE rule_id IS NULL AND contest_message_id IS NULL;

CREATE TABLE roulette_config (
    id smallint PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    config jsonb NOT NULL CHECK (jsonb_typeof(config) = 'object'),
    version integer NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE roulette_tickets (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    order_id bigint REFERENCES orders (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    used_at timestamptz,
    revoked_at timestamptz,
    CONSTRAINT roulette_tickets_order_key UNIQUE (order_id)
);
CREATE INDEX roulette_tickets_open_idx ON roulette_tickets (client_id, id) WHERE used_at IS NULL AND revoked_at IS NULL;

CREATE TABLE roulette_spins (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    source text NOT NULL CHECK (source IN ('free', 'purchase')),
    ticket_id bigint REFERENCES roulette_tickets (id),
    config_version integer NOT NULL,
    sector integer NOT NULL CHECK (sector >= 0),
    prize jsonb NOT NULL CHECK (jsonb_typeof(prize) = 'object'),
    label text NOT NULL,
    balance_minor bigint NOT NULL DEFAULT 0 CHECK (balance_minor >= 0),
    days integer NOT NULL DEFAULT 0 CHECK (days >= 0),
    promo integer NOT NULL DEFAULT 0 CHECK (promo IN (0, 1)),
    state text NOT NULL CHECK (state IN ('empty', 'issuing', 'issued', 'failed')),
    subscription_id bigint REFERENCES subscriptions (id),
    note text,
    code text,
    error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    issued_at timestamptz,
    CONSTRAINT roulette_spins_request_key UNIQUE (client_id, request_id),
    CONSTRAINT roulette_spins_ticket_key UNIQUE (ticket_id)
);
CREATE INDEX roulette_spins_client_idx ON roulette_spins (client_id, id DESC);
CREATE INDEX roulette_spins_time_idx ON roulette_spins (created_at);
