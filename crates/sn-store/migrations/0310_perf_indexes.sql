CREATE INDEX payments_paid_client_idx ON payments (client_id, paid_at)
    INCLUDE (currency, amount_minor, refunded_minor, provider, pay_method)
    WHERE status IN ('paid', 'partially_refunded', 'refunded') AND provider <> 'balance';

CREATE INDEX orders_tariff_sales_idx ON orders ((COALESCE(paid_at, created_at)))
    WHERE kind IN ('new', 'renewal', 'change') AND status IN ('paid', 'fulfilled') AND snapshot IS NOT NULL;

CREATE INDEX ticket_messages_unread_idx ON ticket_messages (ticket_id)
    WHERE author = 'client' AND read_at IS NULL;

CREATE INDEX tickets_queue_idx ON tickets (status, (COALESCE(last_message_id, 0)) DESC, id DESC);

CREATE INDEX tickets_waiting_queue_idx ON tickets ((COALESCE(waiting_message_id, last_message_id, 0)), id)
    WHERE status = 'open' AND last_author = 'client';

CREATE INDEX orders_promo_paid_idx ON orders (paid_at)
    WHERE status IN ('paid', 'fulfilled', 'review')
    AND quote -> 'lines' @> '[{"kind": {"type": "adjustment", "source": {"type": "promo"}}}]';

ANALYZE orders, tickets;
