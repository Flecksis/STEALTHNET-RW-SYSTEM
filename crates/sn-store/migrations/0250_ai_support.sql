ALTER TABLE tickets
    ADD COLUMN ai_state text CHECK (ai_state IN ('active', 'handed_off')),
    ADD COLUMN ai_reason text CHECK (ai_reason IN ('human', 'money', 'unsure', 'groq', 'client_limit', 'budget', 'operator', 'disabled')),
    ADD COLUMN ai_handoff_at timestamptz,
    ADD COLUMN ai_claimed_until timestamptz;

CREATE INDEX tickets_ai_pending_idx ON tickets (last_message_id)
    WHERE status = 'open' AND ai_state = 'active' AND last_author = 'client';

CREATE TABLE ai_support_log (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ticket_id bigint NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    client_id bigint NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    message_id bigint REFERENCES ticket_messages (id) ON DELETE SET NULL,
    outcome text NOT NULL CHECK (outcome IN ('answered', 'handoff', 'stale')),
    reason text CHECK (reason IN ('human', 'money', 'unsure', 'groq', 'client_limit', 'budget')),
    model text,
    prompt_tokens integer NOT NULL DEFAULT 0 CHECK (prompt_tokens >= 0),
    completion_tokens integer NOT NULL DEFAULT 0 CHECK (completion_tokens >= 0),
    error text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ai_support_log_created_idx ON ai_support_log (created_at);
CREATE INDEX ai_support_log_client_idx ON ai_support_log (client_id, created_at) WHERE outcome = 'answered';
CREATE INDEX ai_support_log_handoff_idx ON ai_support_log (id) WHERE outcome = 'handoff';
