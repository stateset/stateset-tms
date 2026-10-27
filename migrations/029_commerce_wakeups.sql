-- Native commerce shares the existing order-source scheduling column.
-- Keep the canonical envelope, original queue bodies and workspace timestamps.
CREATE OR REPLACE FUNCTION freight_queue_due(queue jsonb, inbound boolean, destination text)
RETURNS timestamptz LANGUAGE plpgsql
SET TimeZone = 'UTC' SET DateStyle = 'ISO, YMD' AS $$
DECLARE
    item jsonb;
    state text;
    raw jsonb;
    due timestamptz;
    earliest timestamptz;
BEGIN
    IF queue IS NULL THEN RETURN NULL; END IF;
    -- Malformed retained data must request a full typed check, never hide work.
    IF jsonb_typeof(queue) IS DISTINCT FROM 'array' THEN
        RETURN '-infinity'::timestamptz;
    END IF;
    FOR item IN SELECT value FROM jsonb_array_elements(queue) LOOP
        IF jsonb_typeof(item) IS DISTINCT FROM 'object'
           OR jsonb_typeof(item->'status') IS DISTINCT FROM 'string' THEN
            RETURN '-infinity'::timestamptz;
        END IF;
        IF NOT inbound THEN
            IF jsonb_typeof(item->'destination') IS DISTINCT FROM 'string' THEN
                RETURN '-infinity'::timestamptz;
            END IF;
            IF item->>'destination' <> destination
               AND NOT (destination = 'stateset-sync-server' AND item->>'destination' = 'stateset-icommerce') THEN CONTINUE; END IF;
        END IF;
        state := item->>'status';
        IF state IN ('pending', 'retry') AND inbound
           OR state IN ('pending_handoff', 'retry') AND NOT inbound THEN
            raw := item->'next_attempt_at';
            IF raw IS NULL OR raw = 'null'::jsonb THEN
                RETURN '-infinity'::timestamptz;
            END IF;
        ELSIF state = 'processing' AND inbound OR state = 'sending' AND NOT inbound THEN
            raw := item->'leased_until';
            -- The existing worker does not reclaim a lease without an expiry.
            IF raw IS NULL OR raw = 'null'::jsonb THEN CONTINUE; END IF;
        ELSE
            CONTINUE;
        END IF;
        IF jsonb_typeof(raw) IS DISTINCT FROM 'string'
           OR (raw #>> '{}') !~ '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$' THEN
            RETURN '-infinity'::timestamptz;
        END IF;
        BEGIN
            due := (raw #>> '{}')::timestamptz;
        EXCEPTION WHEN invalid_datetime_format OR datetime_field_overflow
                       OR invalid_time_zone_displacement_value THEN
            RETURN '-infinity'::timestamptz;
        END;
        earliest := least(earliest, due);
    END LOOP;
    RETURN earliest;
END $$;

-- Only changed deadlines need a row update. The history counter describes
-- business changes and must not advance for this derived-only backfill.
-- ALTER TABLE holds the required exclusive lock until the migration commits;
-- rollback restores trigger state if any step fails.
SET CONSTRAINTS freight_history_refresh IMMEDIATE;
ALTER TABLE freight_workspaces DISABLE TRIGGER freight_history_revision_guard;
WITH queues AS MATERIALIZED (
    SELECT tenant_id,freight_canonical_root(tenant_id,data,'outbox') AS outbound
    FROM freight_workspaces
), projected AS (
    SELECT tenant_id,CASE WHEN outbound IS NULL THEN '-infinity'::timestamptz
        ELSE freight_queue_due(outbound,false,'stateset-sync-server') END AS due
    FROM queues
)
UPDATE freight_workspaces AS workspace SET sync_due_at=projected.due
FROM projected
WHERE workspace.tenant_id=projected.tenant_id
  AND workspace.sync_due_at IS DISTINCT FROM projected.due;
ALTER TABLE freight_workspaces ENABLE TRIGGER freight_history_revision_guard;
SET CONSTRAINTS freight_history_refresh DEFERRED;
