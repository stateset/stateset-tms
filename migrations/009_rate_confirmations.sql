DO $$
BEGIN
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version=6 FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Rate confirmation migration requires freight format 6'; END IF;
END $$;
LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM freight_workspaces WHERE data->'_freight_format' IS DISTINCT FROM '6'::jsonb) THEN
        RAISE EXCEPTION 'Rate confirmation migration found incompatible workspace data';
    END IF;
END $$;
UPDATE freight_storage_format SET version=7 WHERE singleton;
UPDATE freight_workspaces SET data=jsonb_set(data,'{_freight_format}','7'::jsonb);
