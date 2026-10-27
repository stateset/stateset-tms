-- Synthetic provenance is server-owned creation metadata, never inferred from
-- existing business data. Native validation precedes the format21 fence.
DO $$ BEGIN
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version=20 FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Synthetic provenance requires exact freight format 20'; END IF;
END $$;
LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE;

-- FREIGHT_POLICY_VALIDATION

-- All retained tenants are unverified. Data resembling a sample is not proof
-- of origin. Only a new successful seed transaction may set this flag later.
ALTER TABLE freight_workspaces ADD COLUMN synthetic_demo boolean NOT NULL DEFAULT false;
ALTER TABLE freight_workspaces ADD CONSTRAINT freight_demo_tenant_scope CHECK(NOT synthetic_demo OR tenant_id='demo');

CREATE OR REPLACE FUNCTION freight_canonical_header_valid(p_data jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE frame jsonb:=p_data->'frame'; descriptor jsonb; names jsonb;
BEGIN
    IF jsonb_typeof(p_data) IS DISTINCT FROM 'object' OR (p_data->'_freight_format')::text IS DISTINCT FROM '21'
       OR (SELECT count(*) FROM jsonb_object_keys(p_data))<>2
       OR jsonb_typeof(frame) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
    IF (SELECT count(*) FROM jsonb_object_keys(frame))<>5 OR (frame->'version')::text IS DISTINCT FROM '1'
       OR jsonb_typeof(frame->'config') IS DISTINCT FROM 'object'
       OR (frame->'config'->'span')::text IS DISTINCT FROM '64'
       OR (frame->'config'->'target_bytes')::text IS DISTINCT FROM '65536'
       OR (SELECT count(*) FROM jsonb_object_keys(frame->'config'))<>2
       OR jsonb_typeof(frame->'roots') IS DISTINCT FROM 'object'
       OR jsonb_typeof(frame->'root_names') IS DISTINCT FROM 'array'
       OR frame->'blocks' IS DISTINCT FROM '[]'::jsonb THEN RETURN false; END IF;
    SELECT coalesce(jsonb_agg(key ORDER BY key COLLATE "C"),'[]'::jsonb) INTO names FROM jsonb_object_keys(frame->'roots') key;
    IF names IS DISTINCT FROM frame->'root_names' THEN RETURN false; END IF;
    FOR descriptor IN SELECT value FROM jsonb_each(frame->'roots') LOOP
        IF jsonb_typeof(descriptor) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
        IF (SELECT count(*) FROM jsonb_object_keys(descriptor))<>3 OR NOT descriptor ?& ARRAY['kind','size','scalar']
           OR jsonb_typeof(descriptor->'kind') IS DISTINCT FROM 'string'
           OR descriptor->>'kind' NOT IN ('array','object','string','number','boolean','null')
           OR jsonb_typeof(descriptor->'size') IS DISTINCT FROM 'number'
           OR (descriptor->'size')::text !~ '^(0|[1-9][0-9]{0,19})$' THEN RETURN false; END IF;
        IF (descriptor->>'size')::numeric>18446744073709551615 THEN RETURN false; END IF;
        IF descriptor->>'kind' IN ('array','object') THEN
            IF descriptor->'scalar' IS DISTINCT FROM 'null'::jsonb THEN RETURN false; END IF;
        ELSIF (descriptor->'size')::text<>'0' OR jsonb_typeof(descriptor->'scalar') IS DISTINCT FROM descriptor->>'kind' THEN RETURN false;
        END IF;
    END LOOP;
    RETURN frame->'roots'->'_freight_format' = '{"kind":"number","size":0,"scalar":21}'::jsonb
        AND (frame->'roots'->'_freight_format'->'scalar')::text='21';
END $$;

CREATE OR REPLACE FUNCTION freight_require_storage_format() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_version integer;
BEGIN
    SELECT version INTO current_version FROM freight_storage_format WHERE singleton;
    IF current_version IS DISTINCT FROM 21 OR NOT coalesce(freight_canonical_header_valid(NEW.data),false) THEN
        RAISE EXCEPTION 'Incompatible freight writer; canonical workspace update refused' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION freight_require_block_writer() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version=21 FOR SHARE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Incompatible freight block writer' USING ERRCODE='23514'; END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF; RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION freight_next_tender_deadline(workspace jsonb) RETURNS timestamptz
LANGUAGE plpgsql SET TimeZone='UTC' SET DateStyle='ISO, YMD' AS $$
DECLARE loads jsonb; item jsonb; raw jsonb; deadline timestamptz; earliest timestamptz;
BEGIN
    IF jsonb_typeof(workspace) IS DISTINCT FROM 'object'
       OR (workspace->'_freight_format')::text IS DISTINCT FROM '21' THEN
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

DO $$ BEGIN
    IF (SELECT count(*) FROM pg_trigger WHERE tgrelid='freight_workspaces'::regclass
        AND NOT tgisinternal AND tgenabled='O' AND tgname=ANY(ARRAY['freight_storage_format_guard','freight_integration_wakeups','freight_load_catalog_refresh','freight_history_revision_guard','freight_history_refresh','freight_tender_deadline_refresh']))<>6 THEN
        RAISE EXCEPTION 'Synthetic provenance requires the enabled canonical workspace guards';
    END IF;
END $$;
ALTER TABLE freight_workspaces DISABLE TRIGGER freight_storage_format_guard;
ALTER TABLE freight_workspaces DISABLE TRIGGER freight_integration_wakeups;
ALTER TABLE freight_workspaces DISABLE TRIGGER freight_load_catalog_refresh;
ALTER TABLE freight_workspaces DISABLE TRIGGER freight_history_revision_guard;
ALTER TABLE freight_workspaces DISABLE TRIGGER freight_history_refresh;
ALTER TABLE freight_workspaces DISABLE TRIGGER freight_tender_deadline_refresh;

-- Only the two compatibility markers change. Do not reconstruct business
-- roots, touch block tuples or files, advance history or reschedule queues.
UPDATE freight_workspaces SET data=jsonb_set(
    jsonb_set(data,'{_freight_format}','21'::jsonb),
    '{frame,roots,_freight_format,scalar}','21'::jsonb);
UPDATE freight_history_state SET source_format='21' WHERE source_format='20';
UPDATE freight_storage_format SET version=21 WHERE singleton;
DO $$ BEGIN
    IF EXISTS(SELECT 1 FROM freight_workspaces WHERE NOT coalesce(freight_canonical_header_valid(data),false))
       OR EXISTS(SELECT 1 FROM freight_workspaces w CROSS JOIN LATERAL jsonb_object_keys(w.data->'frame'->'roots') r
            WHERE NOT freight_canonical_root_valid(w.tenant_id,w.data,r)) THEN
        RAISE EXCEPTION 'Synthetic provenance did not preserve complete canonical metadata';
    END IF;
END $$;
ALTER TABLE freight_workspaces ENABLE TRIGGER freight_storage_format_guard;
ALTER TABLE freight_workspaces ENABLE TRIGGER freight_integration_wakeups;
ALTER TABLE freight_workspaces ENABLE TRIGGER freight_load_catalog_refresh;
ALTER TABLE freight_workspaces ENABLE TRIGGER freight_history_revision_guard;
ALTER TABLE freight_workspaces ENABLE TRIGGER freight_history_refresh;
ALTER TABLE freight_workspaces ENABLE TRIGGER freight_tender_deadline_refresh;
