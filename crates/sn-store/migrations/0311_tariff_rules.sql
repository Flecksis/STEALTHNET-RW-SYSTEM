ALTER TABLE tariff_categories
    ADD COLUMN subscription_limit integer CHECK (subscription_limit BETWEEN 1 AND 100);

ALTER TABLE tariffs
    ADD COLUMN device_discounts jsonb NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(device_discounts) = 'array');
