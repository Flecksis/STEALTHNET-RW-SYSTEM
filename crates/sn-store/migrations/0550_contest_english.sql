ALTER TABLE contest_messages ADD COLUMN text_en text CHECK (text_en IS NULL OR btrim(text_en) <> '');

ALTER TABLE broadcasts ADD COLUMN contest_locale text CHECK (contest_locale IN ('ru', 'en'));
UPDATE broadcasts SET contest_locale = 'ru' WHERE contest_message_id IS NOT NULL;
ALTER TABLE broadcasts ADD CONSTRAINT broadcasts_contest_check CHECK (
    (contest_message_id IS NULL) = (contest_locale IS NULL)
);
DROP INDEX broadcasts_contest_message_key;
CREATE UNIQUE INDEX broadcasts_contest_message_key ON broadcasts (contest_message_id, contest_locale)
    WHERE contest_message_id IS NOT NULL;
