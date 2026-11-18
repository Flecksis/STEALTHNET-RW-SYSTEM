CREATE TABLE remna_node_states (
    node_uuid uuid PRIMARY KEY,
    name text NOT NULL,
    state text NOT NULL CHECK (state IN ('online', 'down', 'disabled')),
    since timestamptz NOT NULL,
    event text,
    message text,
    seen_at timestamptz NOT NULL,
    down_at timestamptz,
    alerted_at timestamptz,
    CHECK (alerted_at IS NULL OR down_at IS NOT NULL)
);
