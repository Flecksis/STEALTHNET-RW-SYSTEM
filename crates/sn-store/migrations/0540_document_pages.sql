CREATE TABLE document_pages (
    id text PRIMARY KEY CHECK (id ~ '^[a-z0-9]{1,32}$'),
    body_ru text NOT NULL,
    body_en text,
    updated_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
