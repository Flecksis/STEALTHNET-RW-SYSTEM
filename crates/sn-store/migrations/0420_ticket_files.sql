CREATE TABLE ticket_files (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ticket_id bigint REFERENCES tickets (id) ON DELETE CASCADE,
    message_id bigint REFERENCES ticket_messages (id) ON DELETE CASCADE,
    author text NOT NULL CHECK (author IN ('client', 'support')),
    client_id bigint REFERENCES clients (id) ON DELETE CASCADE,
    admin_id bigint REFERENCES admins (id) ON DELETE SET NULL,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    mime text NOT NULL CHECK (mime IN ('image/jpeg', 'image/png', 'image/webp', 'image/gif', 'application/pdf', 'text/plain')),
    size integer NOT NULL CHECK (size > 0),
    width integer CHECK (width > 0),
    height integer CHECK (height > 0),
    data bytea,
    link_token text CONSTRAINT ticket_files_link_key UNIQUE,
    link_until timestamptz,
    telegram_sent_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    purged_at timestamptz,
    CHECK ((ticket_id IS NULL) = (message_id IS NULL)),
    CHECK (data IS NOT NULL OR purged_at IS NOT NULL),
    CHECK (author <> 'client' OR client_id IS NOT NULL)
);

CREATE INDEX ticket_files_message_idx ON ticket_files (message_id) WHERE message_id IS NOT NULL;
CREATE INDEX ticket_files_ticket_idx ON ticket_files (ticket_id) WHERE ticket_id IS NOT NULL;
CREATE INDEX ticket_files_pending_idx ON ticket_files (created_at) WHERE message_id IS NULL;
CREATE INDEX ticket_files_client_idx ON ticket_files (client_id, created_at) WHERE client_id IS NOT NULL;
