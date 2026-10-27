-- A tenant aggregate is an intentionally small first-release storage boundary.
-- Mutations lock one tenant row and commit domain data, audit events, EDI handoffs,
-- and idempotency receipts together. No external network I/O occurs in this lock.
CREATE TABLE IF NOT EXISTS freight_workspaces (
    tenant_id TEXT PRIMARY KEY,
    data JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
