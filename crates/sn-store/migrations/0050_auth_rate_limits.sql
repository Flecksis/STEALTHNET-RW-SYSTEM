CREATE TABLE auth_rate_limits (
    bucket bytea NOT NULL,
    window_start timestamptz NOT NULL,
    hits integer NOT NULL DEFAULT 1 CHECK (hits > 0),
    PRIMARY KEY (bucket, window_start)
);
CREATE INDEX auth_rate_limits_window_idx ON auth_rate_limits (window_start);
