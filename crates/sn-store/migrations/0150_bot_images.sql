CREATE TABLE bot_images (
    slot text PRIMARY KEY CHECK (slot IN ('common', 'menu', 'subscription', 'buy', 'balance', 'support', 'promo', 'referral', 'options')),
    content_type text NOT NULL CHECK (content_type IN ('image/png', 'image/jpeg')),
    bytes bytea NOT NULL CHECK (octet_length(bytes) BETWEEN 1 AND 5242880),
    sha256 text NOT NULL,
    updated_by bigint REFERENCES admins (id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
