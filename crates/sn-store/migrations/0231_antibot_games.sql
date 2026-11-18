ALTER TABLE antibot_events DROP CONSTRAINT antibot_events_action_check;
ALTER TABLE antibot_events ADD CONSTRAINT antibot_events_action_check CHECK (action IN (
    'marked', 'asked', 'confirmed', 'trial_denied', 'promo_denied', 'no_bonus', 'cleared',
    'roulette_asked', 'roulette_denied', 'roulette_noted',
    'contest_asked', 'contest_denied', 'contest_noted', 'contest_excluded'
));
