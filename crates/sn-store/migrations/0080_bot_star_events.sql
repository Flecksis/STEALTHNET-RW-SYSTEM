CREATE TABLE bot_star_events (
    charge_id text NOT NULL CHECK (btrim(charge_id) <> ''),
    event text NOT NULL CHECK (event IN ('successful_payment', 'refunded_payment')),
    payload jsonb NOT NULL,
    source text NOT NULL CHECK (source IN ('update', 'reconcile')),
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'done', 'skipped', 'review')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at timestamptz NOT NULL,
    last_error text,
    payment_id bigint REFERENCES payments (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (charge_id, event)
);
CREATE INDEX bot_star_events_due_idx ON bot_star_events (next_attempt_at) WHERE status = 'pending';
CREATE INDEX bot_star_events_attention_idx ON bot_star_events (updated_at DESC) WHERE status = 'review';
CREATE TRIGGER bot_star_events_touch BEFORE UPDATE ON bot_star_events FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
