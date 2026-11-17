CREATE TABLE site_assets (
    name text PRIMARY KEY,
    content_type text NOT NULL CHECK (content_type IN ('image/png', 'image/jpeg', 'image/webp')),
    bytes bytea NOT NULL CHECK (octet_length(bytes) BETWEEN 1 AND 1048576),
    sha256 text NOT NULL,
    updated_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
