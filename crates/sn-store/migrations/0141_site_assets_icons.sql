ALTER TABLE site_assets DROP CONSTRAINT site_assets_content_type_check;

ALTER TABLE site_assets
    ADD CONSTRAINT site_assets_content_type_check
    CHECK (content_type IN ('image/png', 'image/jpeg', 'image/webp', 'image/x-icon'));
