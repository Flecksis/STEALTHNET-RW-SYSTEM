CREATE TABLE client_utm (
    client_id bigint PRIMARY KEY REFERENCES clients (id) ON DELETE CASCADE,
    source text CHECK (length(source) <= 64),
    medium text CHECK (length(medium) <= 64),
    campaign text CHECK (length(campaign) <= 64),
    content text CHECK (length(content) <= 64),
    term text CHECK (length(term) <= 64),
    start_param text NOT NULL,
    registration_source text NOT NULL CHECK (registration_source IN ('bot', 'miniapp', 'web', 'admin', 'import', 'migration')),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT client_utm_any_check CHECK (num_nonnulls(source, medium, campaign, content, term) > 0)
);
CREATE INDEX client_utm_report_idx ON client_utm (created_at, source, medium, campaign);
