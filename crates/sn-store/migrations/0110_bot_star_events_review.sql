ALTER TABLE bot_star_events ADD COLUMN id bigint GENERATED ALWAYS AS IDENTITY;
ALTER TABLE bot_star_events ADD COLUMN resolved_at timestamptz;
ALTER TABLE bot_star_events ADD COLUMN resolved_by bigint REFERENCES admins (id);
ALTER TABLE bot_star_events ADD COLUMN resolution_note text;
ALTER TABLE bot_star_events DROP CONSTRAINT bot_star_events_status_check;
ALTER TABLE bot_star_events ADD CONSTRAINT bot_star_events_status_check CHECK (status IN ('pending', 'done', 'skipped', 'review', 'refunded', 'closed'));
ALTER TABLE bot_star_events ADD CONSTRAINT bot_star_events_resolved_check CHECK ((status IN ('refunded', 'closed')) = (resolved_at IS NOT NULL));
CREATE INDEX bot_star_events_id_idx ON bot_star_events (id);
