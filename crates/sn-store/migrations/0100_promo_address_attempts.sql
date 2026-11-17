CREATE TABLE promo_address_attempts (
    address text NOT NULL CHECK (address <> ''),
    window_start timestamptz NOT NULL,
    failures integer NOT NULL DEFAULT 0 CHECK (failures >= 0),
    PRIMARY KEY (address, window_start)
);
CREATE INDEX promo_address_attempts_window_idx ON promo_address_attempts (window_start);
