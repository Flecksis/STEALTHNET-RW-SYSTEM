ALTER TABLE ext_modules
    ADD COLUMN managed boolean NOT NULL DEFAULT false,
    ADD COLUMN package jsonb,
    ADD COLUMN job jsonb,
    ADD COLUMN health jsonb;
