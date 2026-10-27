DO $$
BEGIN
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version=4 FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Commercial amendment migration requires freight format 4'; END IF;
END $$;
LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM freight_workspaces WHERE data->'_freight_format' IS DISTINCT FROM '4'::jsonb) THEN
        RAISE EXCEPTION 'Commercial amendment migration found incompatible workspace data';
    END IF;
END $$;
UPDATE freight_storage_format SET version=5 WHERE singleton;
UPDATE freight_workspaces SET data=jsonb_set(data,'{_freight_format}','5'::jsonb);
