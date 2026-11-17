CREATE TABLE promo_attempts (
    client_id bigint NOT NULL REFERENCES clients (id),
    window_start timestamptz NOT NULL,
    failures integer NOT NULL DEFAULT 0 CHECK (failures >= 0),
    PRIMARY KEY (client_id, window_start)
);
CREATE INDEX promo_attempts_window_idx ON promo_attempts (window_start);
