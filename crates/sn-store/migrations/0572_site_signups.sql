CREATE TABLE site_signups (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id bigint REFERENCES clients (id) ON DELETE SET NULL,
    via text NOT NULL CHECK (via IN ('password', 'code', 'google', 'apple', 'telegram')),
    address text,
    network text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX site_signups_address_idx ON site_signups (address, created_at) WHERE address IS NOT NULL;
CREATE INDEX site_signups_network_idx ON site_signups (network, created_at) WHERE network IS NOT NULL;
CREATE INDEX site_signups_created_idx ON site_signups (created_at);
CREATE INDEX site_signups_client_idx ON site_signups (client_id) WHERE client_id IS NOT NULL;

CREATE TABLE site_signup_refusals (
    kind text NOT NULL CHECK (kind IN ('address', 'network', 'minute', 'asked')),
    hour timestamptz NOT NULL,
    hits bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, hour)
);

ALTER TABLE oauth_requests ADD COLUMN claims jsonb;
ALTER TABLE oauth_requests DROP CONSTRAINT oauth_requests_status_check;
ALTER TABLE oauth_requests ADD CONSTRAINT oauth_requests_status_check
    CHECK (status IN ('pending', 'human', 'done', 'failed', 'consumed'));
CREATE INDEX oauth_requests_human_idx ON oauth_requests (finished_at) WHERE status = 'human';
