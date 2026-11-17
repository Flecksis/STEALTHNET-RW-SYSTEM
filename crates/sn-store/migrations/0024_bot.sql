CREATE TABLE bot_consents (
    telegram_id bigint NOT NULL CHECK (telegram_id > 0),
    version text NOT NULL CHECK (btrim(version) <> ''),
    documents jsonb NOT NULL DEFAULT '[]'::jsonb,
    accepted_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (telegram_id, version)
);
