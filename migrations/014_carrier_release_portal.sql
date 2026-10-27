DO $$
BEGIN
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version=11 FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Carrier release portal migration requires freight format 11'; END IF;
END $$;
LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM freight_workspaces WHERE data->'_freight_format' IS DISTINCT FROM '11'::jsonb) THEN
        RAISE EXCEPTION 'Carrier release portal migration found incompatible workspace data';
    END IF;
END $$;
UPDATE freight_storage_format SET version=12 WHERE singleton;
UPDATE freight_workspaces SET data=jsonb_set(data,'{_freight_format}','12'::jsonb);
