ALTER TABLE client_identities ADD COLUMN search_text text NOT NULL DEFAULT '';

UPDATE client_identities SET search_text = replace(lower(
    value || ' ' || coalesce(meta ->> 'username', '') || ' ' || coalesce(meta ->> 'first_name', '') || ' ' || coalesce(meta ->> 'last_name', '')
), 'ё', 'е');

CREATE INDEX client_identities_username_idx ON client_identities (lower(meta ->> 'username') text_pattern_ops) WHERE kind = 'telegram';
CREATE INDEX client_identities_email_idx ON client_identities (value text_pattern_ops) WHERE kind = 'email';

CREATE INDEX clients_blocked_idx ON clients (id DESC) WHERE status = 'blocked' AND deleted_at IS NULL;
CREATE INDEX clients_created_at_idx ON clients (created_at) WHERE deleted_at IS NULL;

CREATE INDEX subscriptions_short_uuid_idx ON subscriptions (short_uuid) WHERE short_uuid IS NOT NULL;
CREATE INDEX subscriptions_status_idx ON subscriptions (status, expire_at) WHERE ended_at IS NULL;

CREATE INDEX payments_paid_at_idx ON payments (paid_at) WHERE paid_at IS NOT NULL;
