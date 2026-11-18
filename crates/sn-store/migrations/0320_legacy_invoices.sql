CREATE TABLE legacy_invoices (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source text NOT NULL,
    old_id text NOT NULL,
    provider text NOT NULL,
    old_provider text NOT NULL,
    external_id text,
    order_ref text,
    client_id bigint NOT NULL REFERENCES clients (id),
    intent jsonb NOT NULL,
    label text,
    amount_minor bigint NOT NULL CHECK (amount_minor > 0),
    currency text NOT NULL,
    stars integer,
    pay_url text,
    recurring boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,
    order_id bigint REFERENCES orders (id),
    payment_id bigint REFERENCES payments (id),
    paid_at timestamptz,
    run_id bigint,
    imported_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT legacy_invoices_old_key UNIQUE (source, old_id)
);
CREATE INDEX legacy_invoices_external_idx ON legacy_invoices (provider, external_id) WHERE external_id IS NOT NULL;
CREATE INDEX legacy_invoices_ref_idx ON legacy_invoices (order_ref) WHERE order_ref IS NOT NULL;

ALTER TABLE webhook_inbox ADD COLUMN legacy_path boolean NOT NULL DEFAULT false;
ALTER TABLE webhook_inbox ADD COLUMN legacy_invoice_id bigint REFERENCES legacy_invoices (id);
