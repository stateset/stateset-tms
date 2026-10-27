-- A rebuildable deadline hint avoids decoding quiet tenants every expiry tick.
-- The authoritative timestamp, status, exception and version remain in data.
CREATE TABLE freight_tender_deadlines (
    tenant_id text PRIMARY KEY REFERENCES freight_workspaces(tenant_id) ON DELETE CASCADE,
    source_revision bigint NOT NULL CHECK (source_revision>=0),
    ready boolean NOT NULL DEFAULT false,
    due_at timestamptz
);

CREATE FUNCTION freight_next_tender_deadline(workspace jsonb) RETURNS timestamptz
LANGUAGE plpgsql SET TimeZone='UTC' SET DateStyle='ISO, YMD' AS $$
DECLARE loads jsonb; item jsonb; raw jsonb; deadline timestamptz; earliest timestamptz;
BEGIN
    IF jsonb_typeof(workspace) IS DISTINCT FROM 'object'
       OR (workspace->'_freight_format')::text IS DISTINCT FROM '18' THEN
        RETURN '-infinity'::timestamptz;
    END IF;
    loads:=workspace->'loads';
    IF jsonb_typeof(loads) IS DISTINCT FROM 'array' THEN RETURN '-infinity'::timestamptz; END IF;
    FOR item IN SELECT value FROM jsonb_array_elements(loads) LOOP
        IF jsonb_typeof(item) IS DISTINCT FROM 'object'
           OR jsonb_typeof(item->'status') IS DISTINCT FROM 'string'
           OR item->>'status' NOT IN ('planned','tendered','accepted','dispatched','in_transit','delivered','cancelled')
           OR (item->'exception' IS NOT NULL AND jsonb_typeof(item->'exception') NOT IN ('null','string')) THEN
            RETURN '-infinity'::timestamptz;
        END IF;
        IF item->>'status'<>'tendered' OR (item->'exception' IS NOT NULL AND item->'exception'<>'null'::jsonb) THEN CONTINUE; END IF;
        raw:=item->'tender_expires_at';
        IF raw IS NULL OR raw='null'::jsonb THEN CONTINUE; END IF;
        IF jsonb_typeof(raw) IS DISTINCT FROM 'string'
           OR (raw #>> '{}') !~ '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$' THEN
            RETURN '-infinity'::timestamptz;
        END IF;
        -- Chrono retains fractional leap seconds; PostgreSQL normalizes them.
        -- A scalar hint must never round a native due timestamp into the future.
        IF substring(raw #>> '{}' FROM 18 FOR 2)='60' THEN RETURN '-infinity'::timestamptz; END IF;
        BEGIN deadline:=(raw #>> '{}')::timestamptz;
        EXCEPTION WHEN invalid_datetime_format OR datetime_field_overflow OR invalid_time_zone_displacement_value THEN
            RETURN '-infinity'::timestamptz;
        END;
        -- PostgreSQL rounds to microseconds; native deadlines have nanoseconds.
        -- Err early, then recheck the exact timestamp in the locked native model.
        earliest:=least(earliest,deadline-interval '1 microsecond');
    END LOOP;
    RETURN earliest;
END $$;

CREATE FUNCTION freight_project_tender_deadline(p_tenant text,p_data jsonb,p_revision bigint)
RETURNS void LANGUAGE sql AS $$
    INSERT INTO freight_tender_deadlines AS existing(tenant_id,source_revision,ready,due_at)
    VALUES(p_tenant,p_revision,true,freight_next_tender_deadline(p_data))
    ON CONFLICT(tenant_id) DO UPDATE SET source_revision=EXCLUDED.source_revision,ready=true,due_at=EXCLUDED.due_at
    WHERE (existing.source_revision,existing.ready,existing.due_at) IS DISTINCT FROM (EXCLUDED.source_revision,true,EXCLUDED.due_at);
$$;
-- Avoid source detoasting during JSON ingestion. Invalidation commits with the
-- source; the first expiry preflight builds its hint under the tenant lock.
CREATE FUNCTION freight_refresh_tender_deadline() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM freight_workspaces WHERE tenant_id=NEW.tenant_id AND history_revision=NEW.history_revision) THEN RETURN NEW; END IF;
    INSERT INTO freight_tender_deadlines(tenant_id,source_revision,ready)
    VALUES(NEW.tenant_id,NEW.history_revision,false)
    ON CONFLICT(tenant_id) DO UPDATE SET ready=false;
    RETURN NEW;
END $$;
CREATE TRIGGER freight_tender_deadline_refresh AFTER INSERT OR UPDATE ON freight_workspaces
FOR EACH ROW EXECUTE FUNCTION freight_refresh_tender_deadline();
CREATE FUNCTION freight_ensure_tender_deadline(p_tenant text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source_row freight_workspaces%ROWTYPE;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM freight_workspaces w LEFT JOIN freight_tender_deadlines d USING(tenant_id)
        WHERE w.tenant_id=p_tenant AND (d.tenant_id IS NULL OR NOT d.ready OR d.source_revision<>w.history_revision)) THEN RETURN; END IF;
    SELECT * INTO source_row FROM freight_workspaces WHERE tenant_id=p_tenant FOR UPDATE;
    IF NOT FOUND THEN RETURN; END IF;
    IF EXISTS (SELECT 1 FROM freight_tender_deadlines d WHERE d.tenant_id=p_tenant
        AND d.ready AND d.source_revision=source_row.history_revision) THEN RETURN; END IF;
    PERFORM freight_project_tender_deadline(p_tenant,source_row.data,source_row.history_revision);
END $$;

-- Preserve source JSON, xmin, timestamps, receipts and all business versions.
SELECT freight_project_tender_deadline(tenant_id,data,history_revision) FROM freight_workspaces;
