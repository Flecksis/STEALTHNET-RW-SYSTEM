CREATE TABLE client_passwords (
    client_id bigint PRIMARY KEY REFERENCES clients (id) ON DELETE CASCADE,
    password_hash text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
