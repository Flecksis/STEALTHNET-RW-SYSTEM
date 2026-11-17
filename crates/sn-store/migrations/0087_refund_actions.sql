ALTER TABLE provider_refunds
    ADD COLUMN action_status text NOT NULL DEFAULT 'none' CHECK (action_status IN ('none', 'pending', 'done', 'failed')),
    ADD COLUMN action_attempts integer NOT NULL DEFAULT 0 CHECK (action_attempts >= 0),
    ADD COLUMN action_next_at timestamptz,
    ADD COLUMN action_error text,
    ADD COLUMN action_base_expire_at timestamptz,
    ADD COLUMN action_target_expire_at timestamptz,
    ADD CONSTRAINT provider_refunds_action_next_check CHECK ((action_status = 'pending') = (action_next_at IS NOT NULL));
CREATE INDEX provider_refunds_action_due_idx ON provider_refunds (action_next_at) WHERE action_status = 'pending';
CREATE INDEX provider_refunds_action_failed_idx ON provider_refunds (id) WHERE action_status = 'failed';
