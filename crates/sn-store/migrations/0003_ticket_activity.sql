ALTER TABLE tickets ADD COLUMN last_message_id bigint, ADD COLUMN waiting_message_id bigint;

UPDATE tickets t SET last_message_id = m.last_id
FROM (SELECT ticket_id, max(id) AS last_id FROM ticket_messages GROUP BY ticket_id) m
WHERE m.ticket_id = t.id;

UPDATE tickets t SET last_author = m.author
FROM ticket_messages m
WHERE m.id = t.last_message_id;

UPDATE tickets t SET waiting_message_id = coalesce((
    SELECT min(m.id) FROM ticket_messages m
    WHERE m.ticket_id = t.id AND m.author = 'client'
      AND m.id > coalesce((SELECT max(r.id) FROM ticket_messages r WHERE r.ticket_id = t.id AND r.author <> 'client'), 0)
), t.last_message_id)
WHERE t.last_author = 'client';

CREATE INDEX tickets_waiting_idx ON tickets (waiting_message_id) WHERE status = 'open' AND last_author = 'client';
CREATE INDEX tickets_activity_idx ON tickets (status, last_message_id DESC);
