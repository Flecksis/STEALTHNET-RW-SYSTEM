ALTER TABLE domain_events ADD COLUMN retry_at timestamptz;

CREATE TABLE event_deliveries (
    event_id bigint NOT NULL REFERENCES domain_events (id),
    module text NOT NULL,
    status text NOT NULL CHECK (status IN ('pending', 'done', 'skipped', 'review')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at timestamptz,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (event_id, module),
    CONSTRAINT event_deliveries_pending_check CHECK (status <> 'pending' OR next_attempt_at IS NOT NULL)
);
CREATE INDEX event_deliveries_pending_idx ON event_deliveries (module, event_id) WHERE status = 'pending';
CREATE INDEX event_deliveries_review_idx ON event_deliveries (updated_at DESC) WHERE status = 'review';
