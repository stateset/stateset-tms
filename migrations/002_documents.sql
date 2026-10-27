CREATE TABLE IF NOT EXISTS freight_document_content (
    tenant_id text NOT NULL REFERENCES freight_workspaces(tenant_id),
    document_id text NOT NULL,
    content bytea NOT NULL CHECK (octet_length(content) BETWEEN 1 AND 10485760),
    PRIMARY KEY (tenant_id, document_id)
);
