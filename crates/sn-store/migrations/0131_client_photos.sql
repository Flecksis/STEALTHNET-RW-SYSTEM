CREATE TABLE client_photos (
    client_id bigint PRIMARY KEY REFERENCES clients (id) ON DELETE CASCADE,
    file_unique_id text,
    content bytea,
    content_type text,
    due_at timestamptz,
    checked_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT client_photos_complete CHECK (
        (content IS NULL AND file_unique_id IS NULL AND content_type IS NULL)
        OR (content IS NOT NULL AND file_unique_id IS NOT NULL AND content_type IS NOT NULL)
    ),
    CONSTRAINT client_photos_size CHECK (content IS NULL OR octet_length(content) <= 1048576),
    CONSTRAINT client_photos_type CHECK (content_type IS NULL OR content_type IN ('image/jpeg', 'image/png', 'image/webp'))
);
CREATE INDEX client_photos_due_idx ON client_photos (due_at) WHERE due_at IS NOT NULL;
