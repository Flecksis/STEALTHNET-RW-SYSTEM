ALTER TABLE referral_withdrawals DROP CONSTRAINT referral_withdrawals_method_check;
ALTER TABLE referral_withdrawals ADD COLUMN method_title text;
