CREATE TABLE admin_login_addresses (
    admin_id bigint NOT NULL REFERENCES admins (id) ON DELETE CASCADE,
    address text NOT NULL CHECK (address <> ''),
    last_login_at timestamptz NOT NULL,
    PRIMARY KEY (admin_id, address)
);
