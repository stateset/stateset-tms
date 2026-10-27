-- Stopped request receipts change replay semantics; older releases must not
-- interpret their empty response as a successful business application.
DO $$
BEGIN
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version=17 FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Save resolution migration requires freight format 17'; END IF;
END $$;
LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM freight_workspaces WHERE data->'_freight_format' IS DISTINCT FROM '17'::jsonb) THEN
        RAISE EXCEPTION 'Save resolution migration found incompatible workspace data';
    END IF;
    IF EXISTS (
        SELECT 1 FROM freight_workspaces w,
        LATERAL jsonb_each(CASE WHEN jsonb_typeof(w.data->'receipts')='object' THEN w.data->'receipts' ELSE '{}'::jsonb END) receipt
        WHERE jsonb_typeof(receipt.value)='object' AND receipt.value ? 'stopped'
          AND receipt.value->'stopped' IS DISTINCT FROM 'null'::jsonb
    ) THEN
        RAISE EXCEPTION 'Save resolution migration refuses preexisting stopped receipt data';
    END IF;
END $$;
UPDATE freight_storage_format SET version=18 WHERE singleton;
UPDATE freight_workspaces SET data=jsonb_set(data,'{_freight_format}','18'::jsonb);
