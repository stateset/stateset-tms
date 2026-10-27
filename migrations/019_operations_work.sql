DO $$
BEGIN
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version=16 FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Operations work migration requires freight format 16'; END IF;
END $$;
LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM freight_workspaces WHERE data->'_freight_format' IS DISTINCT FROM '16'::jsonb) THEN
        RAISE EXCEPTION 'Operations work migration found incompatible workspace data';
    END IF;
END $$;
UPDATE freight_storage_format SET version=17 WHERE singleton;
UPDATE freight_workspaces SET data=jsonb_set(data,'{_freight_format}','17'::jsonb);
