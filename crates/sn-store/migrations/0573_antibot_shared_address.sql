ALTER TABLE antibot_events DROP CONSTRAINT antibot_events_action_check;
ALTER TABLE antibot_events ADD CONSTRAINT antibot_events_action_check CHECK (action IN (
    'marked', 'asked', 'confirmed', 'trial_denied', 'promo_denied', 'no_bonus', 'cleared',
    'roulette_asked', 'roulette_denied', 'roulette_noted',
    'contest_asked', 'contest_denied', 'contest_noted', 'contest_excluded',
    'promo_link_asked', 'promo_link_denied', 'shared_address'
)) NOT VALID;
ALTER TABLE antibot_events VALIDATE CONSTRAINT antibot_events_action_check;
