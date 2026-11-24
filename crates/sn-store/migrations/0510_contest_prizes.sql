ALTER TABLE contest_entries DROP CONSTRAINT contest_entries_prize_state_check;
ALTER TABLE contest_entries ADD CONSTRAINT contest_entries_prize_state_check
    CHECK (prize_state IN ('pending', 'waiting', 'issuing', 'issued', 'failed', 'manual'));
ALTER TABLE contest_entries ADD COLUMN prize_by bigint REFERENCES admins (id) ON DELETE SET NULL;
CREATE INDEX contest_entries_manual_idx ON contest_entries (contest_id) WHERE prize_state = 'manual';
