CREATE INDEX trial_claims_client_idx ON trial_claims (client_id) WHERE client_id IS NOT NULL;
CREATE INDEX client_identities_telegram_number_idx ON client_identities ((value::bigint))
    WHERE kind = 'telegram' AND value ~ '^[0-9]{1,18}$';

WITH setup AS MATERIALIZED (
    SELECT COALESCE((SELECT CASE WHEN config->>'young_days' ~ '^[0-9]{1,4}$' THEN (config->>'young_days')::int END
                     FROM module_states WHERE module = 'antibot'), 30) AS days,
           (SELECT min(marked_at) FROM antibot_marks WHERE reasons ? 'mass_invites') AS since
), bound AS MATERIALIZED (
    SELECT max(i.value::bigint) AS number
    FROM client_identities i JOIN clients c ON c.id = i.client_id CROSS JOIN setup s
    WHERE i.kind = 'telegram' AND i.value ~ '^[0-9]{1,18}$'
      AND CASE WHEN c.registration_source IN ('admin', 'import', 'migration') THEN c.created_at
               ELSE i.created_at END <= s.since - make_interval(days => s.days)
), mistaken AS (
    SELECT m.client_id, m.reasons - 'young_account' - 'no_username' - 'burst' - 'mass_invites' AS rest
    FROM antibot_marks m JOIN clients c ON c.id = m.client_id
    WHERE c.registration_source IN ('admin', 'import', 'migration') AND m.cleared_at IS NULL
      AND m.reasons ?| ARRAY['young_account', 'no_username', 'burst', 'mass_invites']
    UNION ALL
    SELECT m.client_id, m.reasons - 'mass_invites'
    FROM antibot_marks m JOIN clients c ON c.id = m.client_id
    WHERE c.registration_source NOT IN ('admin', 'import', 'migration') AND m.cleared_at IS NULL
      AND m.reasons ? 'mass_invites' AND NOT m.reasons ? 'young_account'
      AND NOT EXISTS (
          SELECT 1 FROM client_identities i CROSS JOIN bound b
          WHERE i.client_id = m.client_id AND i.kind = 'telegram'
            AND CASE WHEN i.value ~ '^[0-9]{1,18}$' THEN i.value::bigint > COALESCE(b.number, 0)
                     ELSE false END)
), trimmed AS (
    UPDATE antibot_marks m SET reasons = f.rest FROM mistaken f
    WHERE f.client_id = m.client_id AND f.rest <> '[]'::jsonb
), gone AS (
    DELETE FROM antibot_marks m USING mistaken f
    WHERE f.client_id = m.client_id AND f.rest = '[]'::jsonb RETURNING m.client_id
)
UPDATE antibot_events e SET dedupe = NULL FROM gone
WHERE gone.client_id = e.client_id AND e.action = 'marked';
