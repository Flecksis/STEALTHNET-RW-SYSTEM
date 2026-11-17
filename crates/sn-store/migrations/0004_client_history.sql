CREATE INDEX audit_log_actor_idx ON audit_log (actor_kind, actor_id, id DESC) WHERE actor_id IS NOT NULL;
