CREATE INDEX oauth_requests_session_idx ON oauth_requests (session_id) WHERE session_id IS NOT NULL;
