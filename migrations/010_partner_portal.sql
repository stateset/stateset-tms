DO $$
BEGIN
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version=7 FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Partner portal migration requires freight format 7'; END IF;
END $$;
LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM freight_workspaces WHERE data->'_freight_format' IS DISTINCT FROM '7'::jsonb) THEN
        RAISE EXCEPTION 'Partner portal migration found incompatible workspace data';
    END IF;
END $$;
UPDATE freight_storage_format SET version=8 WHERE singleton;
UPDATE freight_workspaces SET data=jsonb_set(data,'{_freight_format}','8'::jsonb);
