CREATE TABLE tariff_channels (
    tariff_id bigint PRIMARY KEY REFERENCES tariffs (id) ON DELETE CASCADE,
    bot boolean NOT NULL DEFAULT true,
    cabinet boolean NOT NULL DEFAULT true,
    miniapp boolean NOT NULL DEFAULT true,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT tariff_channels_any CHECK (bot OR cabinet OR miniapp)
);
