ALTER TABLE balance_entries DROP CONSTRAINT balance_entries_kind_check;
ALTER TABLE balance_entries ADD CONSTRAINT balance_entries_kind_check CHECK (kind IN ('topup', 'purchase', 'autorenew', 'refund', 'referral_credit', 'referral_reversal', 'withdrawal_hold', 'withdrawal_release', 'contest', 'roulette', 'admin_adjust', 'migration_opening', 'refund_hold', 'refund_release'));
ALTER TABLE provider_refunds ADD COLUMN hold_entry_id bigint REFERENCES balance_entries (id);
