ALTER TABLE auto_broadcasts RENAME COLUMN days TO amount;
ALTER TABLE auto_broadcasts DROP CONSTRAINT auto_broadcasts_days_check;
ALTER TABLE auto_broadcasts ADD COLUMN unit text NOT NULL DEFAULT 'days' CHECK (unit IN ('hours', 'days'));
ALTER TABLE auto_broadcasts ADD CONSTRAINT auto_broadcasts_amount_check CHECK (
    amount >= 0 AND amount <= CASE unit WHEN 'hours' THEN 168 ELSE 90 END
);
ALTER TABLE auto_broadcasts DROP CONSTRAINT auto_broadcasts_trigger_check;
ALTER TABLE auto_broadcasts ADD CONSTRAINT auto_broadcasts_trigger_check CHECK (
    trigger IN ('expiring', 'expired', 'trial_ended', 'no_purchase', 'not_connected', 'no_subscription',
                'trial_expiring', 'traffic_low')
);
ALTER TABLE auto_broadcasts ADD COLUMN percent smallint CHECK (percent BETWEEN 10 AND 99);
ALTER TABLE auto_broadcasts ADD CONSTRAINT auto_broadcasts_percent_needed CHECK (trigger <> 'traffic_low' OR percent IS NOT NULL);
ALTER TABLE auto_broadcasts ADD COLUMN en jsonb CHECK (en IS NULL OR jsonb_typeof(en) = 'object');
ALTER TABLE auto_broadcasts ADD COLUMN preset text CHECK (preset IS NULL OR preset <> '');
CREATE UNIQUE INDEX auto_broadcasts_preset_key ON auto_broadcasts (preset) WHERE preset IS NOT NULL;

ALTER TABLE broadcasts ADD COLUMN rule_locale text CHECK (rule_locale IN ('ru', 'en'));
UPDATE broadcasts SET rule_locale = 'ru' WHERE rule_id IS NOT NULL;
ALTER TABLE broadcasts DROP CONSTRAINT broadcasts_rule_check;
ALTER TABLE broadcasts ADD CONSTRAINT broadcasts_rule_check CHECK (
    (rule_id IS NULL AND rule_day IS NULL AND rule_version IS NULL AND rule_locale IS NULL)
    OR (rule_id IS NOT NULL AND rule_day IS NOT NULL AND rule_version IS NOT NULL AND rule_locale IS NOT NULL)
);
DROP INDEX broadcasts_rule_day_key;
CREATE UNIQUE INDEX broadcasts_rule_day_key ON broadcasts (rule_id, rule_day, rule_version, rule_locale) WHERE rule_id IS NOT NULL;

CREATE TABLE auto_broadcast_traffic (
    rule_id bigint NOT NULL REFERENCES auto_broadcasts (id) ON DELETE CASCADE,
    subscription_id bigint NOT NULL REFERENCES subscriptions (id) ON DELETE CASCADE,
    seq integer NOT NULL CHECK (seq > 0),
    low boolean NOT NULL DEFAULT false,
    PRIMARY KEY (rule_id, subscription_id)
);
CREATE INDEX subscriptions_traffic_limited_idx ON subscriptions (id) WHERE ended_at IS NULL AND traffic_limit_bytes > 0;
