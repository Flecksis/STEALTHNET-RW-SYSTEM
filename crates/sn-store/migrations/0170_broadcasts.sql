CREATE TABLE broadcast_images (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    content_type text NOT NULL CHECK (content_type IN ('image/png', 'image/jpeg')),
    bytes bytea NOT NULL CHECK (octet_length(bytes) BETWEEN 1 AND 5242880),
    width integer NOT NULL CHECK (width > 0),
    height integer NOT NULL CHECK (height > 0),
    file_bot bigint,
    file_id text,
    created_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT broadcast_images_file_check CHECK ((file_id IS NULL) = (file_bot IS NULL))
);

CREATE TABLE broadcasts (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text text NOT NULL CHECK (btrim(text) <> ''),
    image_id bigint REFERENCES broadcast_images (id),
    buttons jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(buttons) = 'array'),
    audience jsonb NOT NULL CHECK (jsonb_typeof(audience) = 'object'),
    status text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'scheduled', 'sending', 'done', 'canceled')),
    send_at timestamptz,
    started_at timestamptz,
    finished_at timestamptz,
    total integer NOT NULL DEFAULT 0 CHECK (total >= 0),
    notice text,
    created_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    started_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT broadcasts_schedule_check CHECK (status <> 'scheduled' OR send_at IS NOT NULL)
);
CREATE INDEX broadcasts_due_idx ON broadcasts (send_at) WHERE status = 'scheduled';
CREATE INDEX broadcasts_sending_idx ON broadcasts (started_at, id) WHERE status = 'sending';
CREATE TRIGGER broadcasts_touch BEFORE UPDATE ON broadcasts FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TABLE broadcast_recipients (
    broadcast_id bigint NOT NULL REFERENCES broadcasts (id) ON DELETE CASCADE,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    chat_id bigint NOT NULL,
    state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'sending', 'sent', 'failed', 'blocked', 'skipped')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    not_before timestamptz,
    claimed_at timestamptz,
    done_at timestamptz,
    error text,
    PRIMARY KEY (broadcast_id, client_id),
    CONSTRAINT broadcast_recipients_chat_key UNIQUE (broadcast_id, chat_id)
);
CREATE INDEX broadcast_recipients_queue_idx ON broadcast_recipients (broadcast_id, client_id) WHERE state = 'pending';
CREATE INDEX broadcast_recipients_flight_idx ON broadcast_recipients (broadcast_id) WHERE state = 'sending';
