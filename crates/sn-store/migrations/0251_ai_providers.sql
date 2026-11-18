ALTER TABLE tickets DROP CONSTRAINT tickets_ai_reason_check;
UPDATE tickets SET ai_reason = 'no_answer' WHERE ai_reason = 'groq';
ALTER TABLE tickets ADD CONSTRAINT tickets_ai_reason_check
    CHECK (ai_reason IN ('human', 'money', 'unsure', 'no_answer', 'groq', 'client_limit', 'budget', 'operator', 'disabled'));

ALTER TABLE ai_support_log DROP CONSTRAINT ai_support_log_reason_check;
UPDATE ai_support_log SET reason = 'no_answer' WHERE reason = 'groq';
ALTER TABLE ai_support_log ADD CONSTRAINT ai_support_log_reason_check
    CHECK (reason IN ('human', 'money', 'unsure', 'no_answer', 'groq', 'client_limit', 'budget'));
