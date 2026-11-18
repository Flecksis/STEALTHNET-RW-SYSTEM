ALTER TABLE promo_codes ADD COLUMN owner_client_id bigint REFERENCES clients (id);

UPDATE promo_codes p
SET owner_client_id = substring(p.note from 'клиент #([0-9]+)')::bigint
WHERE p.code LIKE 'WIN-%'
  AND p.note ~ 'клиент #[0-9]+'
  AND EXISTS (
      SELECT 1 FROM clients c WHERE c.id = substring(p.note from 'клиент #([0-9]+)')::bigint
  );

ALTER TABLE client_passwords ADD COLUMN must_change boolean NOT NULL DEFAULT false;

ALTER TABLE client_sessions DROP CONSTRAINT client_sessions_scope_check;
ALTER TABLE client_sessions ADD CONSTRAINT client_sessions_scope_check
    CHECK (scope IN ('full', 'subscription_view', 'password_change'));

ALTER TABLE client_login_requests ADD COLUMN password_change boolean NOT NULL DEFAULT false;
