ALTER TABLE broadcasts ADD COLUMN channel text NOT NULL DEFAULT 'bot' CHECK (channel IN ('bot', 'email', 'both'));
ALTER TABLE broadcasts ADD COLUMN subject text;
ALTER TABLE broadcasts ADD COLUMN mail_notice text;

ALTER TABLE broadcast_images ADD COLUMN public_key uuid NOT NULL DEFAULT gen_random_uuid();
ALTER TABLE broadcast_images ADD CONSTRAINT broadcast_images_public_key UNIQUE (public_key);

ALTER TABLE broadcast_recipients ADD COLUMN channel text NOT NULL DEFAULT 'bot' CHECK (channel IN ('bot', 'email'));
ALTER TABLE broadcast_recipients ADD COLUMN email text;
ALTER TABLE broadcast_recipients ADD COLUMN unsubscribe uuid;
ALTER TABLE broadcast_recipients ALTER COLUMN chat_id DROP NOT NULL;
ALTER TABLE broadcast_recipients DROP CONSTRAINT broadcast_recipients_pkey;
ALTER TABLE broadcast_recipients ADD PRIMARY KEY (broadcast_id, channel, client_id);
ALTER TABLE broadcast_recipients ADD CONSTRAINT broadcast_recipients_target_check CHECK (
    (channel = 'bot' AND chat_id IS NOT NULL AND email IS NULL)
    OR (channel = 'email' AND email IS NOT NULL AND chat_id IS NULL AND unsubscribe IS NOT NULL)
);
ALTER TABLE broadcast_recipients ADD CONSTRAINT broadcast_recipients_email_key UNIQUE (broadcast_id, email);
CREATE UNIQUE INDEX broadcast_recipients_unsubscribe_key ON broadcast_recipients (unsubscribe);

DROP INDEX broadcast_recipients_queue_idx;
CREATE INDEX broadcast_recipients_queue_idx ON broadcast_recipients (broadcast_id, channel, client_id) WHERE state = 'pending';
DROP INDEX broadcast_recipients_flight_idx;
CREATE INDEX broadcast_recipients_flight_idx ON broadcast_recipients (channel, broadcast_id) WHERE state = 'sending';

CREATE TABLE broadcast_unsubscribes (
    email text PRIMARY KEY CHECK (email = lower(btrim(email)) AND email <> ''),
    client_id bigint REFERENCES clients (id) ON DELETE SET NULL,
    broadcast_id bigint REFERENCES broadcasts (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
