-- Derived scheduling metadata. The JSON remains the authoritative queue and
-- every claim still rechecks its current lease under the tenant row lock.
ALTER TABLE freight_workspaces ADD COLUMN inbox_due_at timestamptz;
ALTER TABLE freight_workspaces ADD COLUMN edi_due_at timestamptz;
ALTER TABLE freight_workspaces ADD COLUMN sync_due_at timestamptz;

CREATE FUNCTION freight_queue_due(queue jsonb, inbound boolean, destination text)
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
            IF item->>'destination' <> destination THEN CONTINUE; END IF;
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

CREATE FUNCTION freight_refresh_integration_wakeups() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    NEW.inbox_due_at := freight_queue_due(NEW.data->'inbox', true, NULL);
    NEW.edi_due_at := freight_queue_due(NEW.data->'outbox', false, 'stateset-edi');
    NEW.sync_due_at := freight_queue_due(NEW.data->'outbox', false, 'stateset-sync-server');
    RETURN NEW;
END $$;

CREATE TRIGGER freight_integration_wakeups
BEFORE INSERT OR UPDATE OF data, inbox_due_at, edi_due_at, sync_due_at
ON freight_workspaces FOR EACH ROW
EXECUTE FUNCTION freight_refresh_integration_wakeups();

-- Touch only derived metadata: preserve original JSON and updated_at exactly.
UPDATE freight_workspaces SET inbox_due_at = NULL;
