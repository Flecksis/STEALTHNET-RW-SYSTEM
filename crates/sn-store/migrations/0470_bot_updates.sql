CREATE TABLE bot_updates (
    bot_id bigint NOT NULL,
    update_id bigint NOT NULL,
    payload jsonb,
    received_at timestamptz NOT NULL DEFAULT now(),
    taken_at timestamptz,
    PRIMARY KEY (bot_id, update_id),
    CHECK (payload IS NOT NULL OR taken_at IS NOT NULL)
);

CREATE INDEX bot_updates_pending_idx ON bot_updates (bot_id, update_id) WHERE taken_at IS NULL;
CREATE INDEX bot_updates_received_idx ON bot_updates (received_at);
