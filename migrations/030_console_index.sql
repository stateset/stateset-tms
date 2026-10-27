-- Rebuildable terminal read cache. Originals and writer format remain unchanged.
-- Any canonical write invalidates this cache atomically, including an older
-- running writer. Cold reads rebuild under the tenant's original row lock.
CREATE TABLE freight_console_state (
    tenant_id text PRIMARY KEY REFERENCES freight_workspaces(tenant_id) ON DELETE CASCADE,
    ready boolean NOT NULL DEFAULT false,
    as_of date NOT NULL,
    finance jsonb NOT NULL,
    errors jsonb NOT NULL
);
CREATE TABLE freight_console_records (
    tenant_id text NOT NULL REFERENCES freight_console_state(tenant_id) ON DELETE CASCADE,
    view text COLLATE "C" NOT NULL CHECK(view IN ('billing','payables','integrations')),
    record_key text COLLATE "C" NOT NULL CHECK(octet_length(record_key) BETWEEN 1 AND 202),
    collection text NOT NULL CHECK(collection IN ('bills','settlements','outbox','inbox')),
    summary jsonb NOT NULL,
    search_values text[] NOT NULL,
    balance text NOT NULL CHECK(balance IN ('open','credits','zero','unknown')),
    direction text NOT NULL CHECK(direction IN ('all','outbound','inbound')),
    attention boolean NOT NULL,
    overdue boolean NOT NULL,
    PRIMARY KEY(tenant_id,view,record_key)
);
CREATE INDEX freight_console_balance ON freight_console_records(tenant_id,view,balance,record_key);
CREATE INDEX freight_console_direction ON freight_console_records(tenant_id,view,direction,record_key);
CREATE INDEX freight_console_attention ON freight_console_records(tenant_id,view,attention,record_key);
CREATE INDEX freight_console_overdue ON freight_console_records(tenant_id,view,overdue,record_key);
CREATE FUNCTION freight_invalidate_console() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='TRUNCATE' THEN UPDATE freight_console_state SET ready=false WHERE ready; RETURN NULL; END IF;
    IF TG_OP<>'INSERT' THEN UPDATE freight_console_state SET ready=false WHERE tenant_id=OLD.tenant_id AND ready; END IF;
    IF TG_OP<>'DELETE' THEN UPDATE freight_console_state SET ready=false WHERE tenant_id=NEW.tenant_id AND ready; RETURN NEW; END IF;
    RETURN OLD;
END $$;
CREATE TRIGGER freight_console_header AFTER INSERT OR UPDATE OF data ON freight_workspaces
FOR EACH ROW EXECUTE FUNCTION freight_invalidate_console();
CREATE TRIGGER freight_console_blocks AFTER INSERT OR UPDATE OR DELETE ON freight_workspace_blocks
FOR EACH ROW EXECUTE FUNCTION freight_invalidate_console();
CREATE TRIGGER freight_console_truncate AFTER TRUNCATE ON freight_workspace_blocks
FOR EACH STATEMENT EXECUTE FUNCTION freight_invalidate_console();
