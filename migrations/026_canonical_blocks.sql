-- One transaction owns the format fence, DDL, lossless native backfill and
-- ledger entry. The API executes the marked native backfill between halves.
DO $$ BEGIN
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version=18 FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Canonical storage requires freight format 18'; END IF;
END $$;
LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE;
DROP TRIGGER freight_storage_format_guard ON freight_workspaces;
DROP TRIGGER freight_integration_wakeups ON freight_workspaces;
DROP TRIGGER freight_load_catalog_refresh ON freight_workspaces;
DROP TRIGGER freight_history_revision_guard ON freight_workspaces;
DROP TRIGGER freight_history_refresh ON freight_workspaces;
DROP TRIGGER freight_tender_deadline_refresh ON freight_workspaces;

CREATE TABLE freight_workspace_blocks (
    tenant_id text NOT NULL REFERENCES freight_workspaces(tenant_id) ON DELETE CASCADE,
    root text COLLATE "C" NOT NULL, key text COLLATE "C" NOT NULL,
    payload jsonb NOT NULL,
    PRIMARY KEY(tenant_id,root,key),
    CHECK(coalesce(jsonb_typeof(payload)='object' AND payload->>'root'=root
        AND payload->>'key'=key AND jsonb_typeof(payload->'items')='array'
        AND jsonb_array_length(payload->'items') BETWEEN 1 AND 64,false))
);
CREATE TABLE freight_block_counts (
    tenant_id text NOT NULL REFERENCES freight_workspaces(tenant_id) ON DELETE CASCADE,
    root text COLLATE "C" NOT NULL,
    records bigint NOT NULL CHECK(records>=0), blocks bigint NOT NULL CHECK(blocks>=0),
    CHECK((blocks=0 AND records=0) OR (blocks>0 AND records>=blocks)),
    PRIMARY KEY(tenant_id,root)
);

-- FREIGHT_CANONICAL_BACKFILL

CREATE FUNCTION freight_canonical_header_valid(p_data jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE frame jsonb:=p_data->'frame'; descriptor jsonb; names jsonb;
BEGIN
    IF jsonb_typeof(p_data) IS DISTINCT FROM 'object' OR (p_data->'_freight_format')::text IS DISTINCT FROM '19'
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
    RETURN frame->'roots'->'_freight_format' = '{"kind":"number","size":0,"scalar":19}'::jsonb
        AND (frame->'roots'->'_freight_format'->'scalar')::text='19';
END $$;

INSERT INTO freight_block_counts
SELECT tenant_id,root,sum(jsonb_array_length(payload->'items')),count(*) FROM freight_workspace_blocks GROUP BY tenant_id,root;

CREATE FUNCTION freight_canonical_root_valid(p_tenant text,p_header jsonb,p_root text) RETURNS boolean
LANGUAGE plpgsql STABLE AS $$
DECLARE descriptor jsonb; actual freight_block_counts%ROWTYPE;
BEGIN
    IF NOT coalesce(freight_canonical_header_valid(p_header),false) THEN RETURN false; END IF;
    descriptor:=p_header->'frame'->'roots'->p_root;
    IF descriptor IS NULL THEN RETURN false; END IF;
    SELECT * INTO actual FROM freight_block_counts WHERE tenant_id=p_tenant AND root=p_root COLLATE "C";
    IF descriptor->>'kind' IN ('array','object') THEN
        RETURN coalesce(actual.records,0)=(descriptor->>'size')::numeric
            AND (coalesce(actual.records,0)=0 AND coalesce(actual.blocks,0)=0
                 OR actual.blocks>0 AND actual.blocks<=actual.records);
    END IF;
    RETURN coalesce(actual.records,0)=0 AND coalesce(actual.blocks,0)=0;
END $$;
CREATE FUNCTION freight_canonical_root(p_tenant text,p_header jsonb,p_root text) RETURNS jsonb
LANGUAGE plpgsql STABLE AS $$
DECLARE descriptor jsonb; result jsonb; expected numeric;
BEGIN
    IF NOT freight_canonical_root_valid(p_tenant,p_header,p_root) THEN RETURN NULL; END IF;
    descriptor:=p_header->'frame'->'roots'->p_root; expected:=(descriptor->>'size')::numeric;
    IF descriptor->>'kind' NOT IN ('array','object') THEN RETURN descriptor->'scalar'; END IF;
    IF EXISTS (SELECT 1 FROM freight_workspace_blocks b CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') e
        WHERE b.tenant_id=p_tenant AND b.root=p_root COLLATE "C" AND
        (jsonb_typeof(e) IS DISTINCT FROM 'object' OR NOT e ?& ARRAY['position','name','payload']
         OR jsonb_typeof(e->'name') IS DISTINCT FROM 'string' OR jsonb_typeof(e->'position') IS DISTINCT FROM 'number'
         OR (e->'position')::text !~ '^(0|[1-9][0-9]{0,19})$')) THEN RETURN NULL; END IF;
    IF descriptor->>'kind'='array' THEN
        IF EXISTS (SELECT 1 FROM freight_workspace_blocks b CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') e
            WHERE b.tenant_id=p_tenant AND b.root=p_root COLLATE "C" AND (e->>'name'<>'' OR (e->>'position')::numeric<1 OR (e->>'position')::numeric>expected))
           OR (SELECT count(DISTINCT (e->>'position')::numeric) FROM freight_workspace_blocks b CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') e
               WHERE b.tenant_id=p_tenant AND b.root=p_root COLLATE "C")<>expected THEN RETURN NULL; END IF;
        SELECT coalesce(jsonb_agg(e->'payload' ORDER BY (e->>'position')::numeric),'[]'::jsonb) INTO result
        FROM freight_workspace_blocks b CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') e WHERE b.tenant_id=p_tenant AND b.root=p_root COLLATE "C";
    ELSE
        IF EXISTS (SELECT 1 FROM freight_workspace_blocks b CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') e
            WHERE b.tenant_id=p_tenant AND b.root=p_root COLLATE "C" AND (e->'position')::text<>'0')
           OR (SELECT count(DISTINCT (e->>'name') COLLATE "C") FROM freight_workspace_blocks b CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') e
               WHERE b.tenant_id=p_tenant AND b.root=p_root COLLATE "C")<>expected THEN RETURN NULL; END IF;
        SELECT coalesce(jsonb_object_agg(e->>'name',e->'payload'),'{}'::jsonb) INTO result
        FROM freight_workspace_blocks b CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') e WHERE b.tenant_id=p_tenant AND b.root=p_root COLLATE "C";
    END IF;
    RETURN result;
END $$;
-- Only these optional business collections default to empty when their source
-- member is absent. Declared/malformed/missing block members never use defaults.
CREATE FUNCTION freight_canonical_business_root(p_tenant text,p_header jsonb,p_root text) RETURNS jsonb
LANGUAGE plpgsql STABLE AS $$
BEGIN
    IF freight_canonical_header_valid(p_header) AND p_root IN ('appointments','stop_events','inbox')
       AND NOT p_header->'frame'->'roots' ? p_root
       AND NOT EXISTS(SELECT 1 FROM freight_block_counts WHERE tenant_id=p_tenant AND root=p_root COLLATE "C" AND (records<>0 OR blocks<>0)) THEN
        RETURN '[]'::jsonb;
    END IF;
    RETURN freight_canonical_root(p_tenant,p_header,p_root);
END $$;
CREATE FUNCTION freight_canonical_selected(p_tenant text,p_header jsonb,p_roots text[]) RETURNS jsonb
LANGUAGE plpgsql STABLE AS $$
DECLARE result jsonb:=jsonb_build_object('_freight_format',p_header->'_freight_format'); name text; value jsonb;
BEGIN
    FOREACH name IN ARRAY p_roots LOOP
        value:=freight_canonical_business_root(p_tenant,p_header,name);
        result:=result || jsonb_build_object(name,value);
    END LOOP;
    RETURN result;
END $$;
CREATE FUNCTION freight_canonical_record(p_tenant text,p_header jsonb,p_root text,p_position bigint) RETURNS jsonb
LANGUAGE plpgsql STABLE AS $$
DECLARE prefix text; result jsonb; matches bigint;
BEGIN
    IF p_position<1 OR NOT freight_canonical_root_valid(p_tenant,p_header,p_root)
       OR p_header->'frame'->'roots'->p_root->>'kind'<>'array'
       OR p_position>(p_header->'frame'->'roots'->p_root->>'size')::numeric THEN RETURN NULL; END IF;
    prefix:='a/'||lpad(to_hex((p_position-1)/64),16,'0')||'/';
    SELECT count(*),jsonb_agg(e->'payload')->0 INTO matches,result
    FROM freight_workspace_blocks b CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') e
    WHERE b.tenant_id=p_tenant AND b.root=p_root COLLATE "C" AND b.key>=prefix COLLATE "C" AND b.key<(prefix||'~') COLLATE "C"
      AND (e->'position')::text=p_position::text AND e->'name'='""'::jsonb;
    IF matches<>1 THEN RETURN NULL; END IF;
    RETURN result;
END $$;
-- Read-only diagnostics/qualification. Application reads use native streaming
-- block decoding; this function is not a second stored/writable aggregate.
CREATE FUNCTION freight_workspace_originals(p_tenant text) RETURNS jsonb
LANGUAGE plpgsql STABLE AS $$
DECLARE header jsonb; result jsonb:='{}'::jsonb; name text; value jsonb;
BEGIN
    SELECT data INTO header FROM freight_workspaces WHERE tenant_id=p_tenant;
    IF NOT FOUND OR NOT coalesce(freight_canonical_header_valid(header),false) THEN RETURN NULL; END IF;
    FOR name IN SELECT jsonb_object_keys(header->'frame'->'roots') LOOP
        value:=freight_canonical_root(p_tenant,header,name);
        IF value IS NULL THEN RETURN NULL; END IF;
        result:=result||jsonb_build_object(name,value);
    END LOOP;
    RETURN result;
END $$;
CREATE FUNCTION freight_canonical_receipt(p_tenant text,p_header jsonb,p_key text) RETURNS jsonb
LANGUAGE plpgsql STABLE AS $$
DECLARE prefix text; result jsonb; matches bigint;
BEGIN
    IF NOT freight_canonical_root_valid(p_tenant,p_header,'receipts')
       OR p_header->'frame'->'roots'->'receipts'->>'kind'<>'object' THEN RETURN NULL; END IF;
    prefix:='o/'||substr(encode(sha256(convert_to(p_key,'UTF8')),'hex'),1,2)||'/';
    SELECT count(*),jsonb_agg(e->'payload')->0 INTO matches,result
    FROM freight_workspace_blocks b CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') e
    WHERE b.tenant_id=p_tenant AND b.root='receipts' AND b.key>=prefix COLLATE "C" AND b.key<(prefix||'~') COLLATE "C"
      AND (e->'position')::text='0' AND jsonb_typeof(e->'name')='string' AND (e->>'name') COLLATE "C"=p_key COLLATE "C";
    IF matches>1 THEN RAISE EXCEPTION 'Ambiguous original receipt' USING ERRCODE='23514'; END IF;
    RETURN result;
END $$;

CREATE OR REPLACE FUNCTION freight_require_storage_format() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_version integer;
BEGIN
    SELECT version INTO current_version FROM freight_storage_format WHERE singleton;
    IF current_version IS DISTINCT FROM 19 OR NOT coalesce(freight_canonical_header_valid(NEW.data),false) THEN
        RAISE EXCEPTION 'Incompatible freight writer; canonical workspace update refused' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE FUNCTION freight_require_block_writer() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version=19 FOR SHARE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Incompatible freight block writer' USING ERRCODE='23514'; END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF; RETURN NEW;
END $$;
CREATE OR REPLACE FUNCTION freight_refresh_integration_wakeups() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE inbound jsonb; outbound jsonb;
BEGIN
    inbound:=freight_canonical_business_root(NEW.tenant_id,NEW.data,'inbox');
    outbound:=freight_canonical_root(NEW.tenant_id,NEW.data,'outbox');
    NEW.inbox_due_at:=CASE WHEN inbound IS NULL THEN '-infinity'::timestamptz ELSE freight_queue_due(inbound,true,NULL) END;
    NEW.edi_due_at:=CASE WHEN outbound IS NULL THEN '-infinity'::timestamptz ELSE freight_queue_due(outbound,false,'stateset-edi') END;
    NEW.sync_due_at:=CASE WHEN outbound IS NULL THEN '-infinity'::timestamptz ELSE freight_queue_due(outbound,false,'stateset-sync-server') END;
    RETURN NEW;
END $$;
CREATE OR REPLACE FUNCTION freight_refresh_load_catalog() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' AND OLD.data->'frame'->'roots'->'loads' IS NOT DISTINCT FROM NEW.data->'frame'->'roots'->'loads'
       AND EXISTS(SELECT 1 FROM freight_load_catalog WHERE tenant_id=NEW.tenant_id AND valid) THEN RETURN NEW; END IF;
    PERFORM freight_project_load_catalog(NEW.tenant_id,freight_canonical_selected(NEW.tenant_id,NEW.data,ARRAY['loads']));
    RETURN NEW;
END $$;
CREATE FUNCTION freight_ensure_load_catalog(p_tenant text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source_row freight_workspaces%ROWTYPE;
BEGIN
    IF EXISTS(SELECT 1 FROM freight_load_catalog WHERE tenant_id=p_tenant AND valid) THEN RETURN; END IF;
    SELECT * INTO source_row FROM freight_workspaces WHERE tenant_id=p_tenant FOR UPDATE;
    IF NOT FOUND THEN RETURN; END IF;
    PERFORM freight_project_load_catalog(p_tenant,freight_canonical_selected(p_tenant,source_row.data,ARRAY['loads']));
END $$;
CREATE OR REPLACE FUNCTION freight_rebuild_history(p_tenant text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source_row freight_workspaces%ROWTYPE;
BEGIN
    SELECT * INTO source_row FROM freight_workspaces WHERE tenant_id=p_tenant FOR UPDATE;
    IF FOUND THEN PERFORM freight_project_history(p_tenant,
        freight_canonical_selected(p_tenant,source_row.data,ARRAY['loads','events','appointments','stop_events']),source_row.history_revision); END IF;
END $$;
CREATE OR REPLACE FUNCTION freight_ensure_history(p_tenant text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source_row freight_workspaces%ROWTYPE;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM freight_workspaces w LEFT JOIN freight_history_state s USING(tenant_id)
        WHERE w.tenant_id=p_tenant AND (s.tenant_id IS NULL OR NOT s.ready OR s.source_revision<>w.history_revision)) THEN RETURN; END IF;
    SELECT * INTO source_row FROM freight_workspaces WHERE tenant_id=p_tenant FOR UPDATE;
    IF NOT FOUND THEN RETURN; END IF;
    IF EXISTS (SELECT 1 FROM freight_history_state WHERE tenant_id=p_tenant AND ready AND source_revision=source_row.history_revision) THEN RETURN; END IF;
    PERFORM freight_project_history(p_tenant,
        freight_canonical_selected(p_tenant,source_row.data,ARRAY['loads','events','appointments','stop_events']),source_row.history_revision);
END $$;
CREATE OR REPLACE FUNCTION freight_next_tender_deadline(workspace jsonb) RETURNS timestamptz
LANGUAGE plpgsql SET TimeZone='UTC' SET DateStyle='ISO, YMD' AS $$
DECLARE loads jsonb; item jsonb; raw jsonb; deadline timestamptz; earliest timestamptz;
BEGIN
    IF jsonb_typeof(workspace) IS DISTINCT FROM 'object'
       OR (workspace->'_freight_format')::text IS DISTINCT FROM '19' THEN
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


CREATE OR REPLACE FUNCTION freight_ensure_tender_deadline(p_tenant text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source_row freight_workspaces%ROWTYPE;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM freight_workspaces w LEFT JOIN freight_tender_deadlines d USING(tenant_id)
        WHERE w.tenant_id=p_tenant AND (d.tenant_id IS NULL OR NOT d.ready OR d.source_revision<>w.history_revision)) THEN RETURN; END IF;
    SELECT * INTO source_row FROM freight_workspaces WHERE tenant_id=p_tenant FOR UPDATE;
    IF NOT FOUND THEN RETURN; END IF;
    IF EXISTS(SELECT 1 FROM freight_tender_deadlines WHERE tenant_id=p_tenant AND ready AND source_revision=source_row.history_revision) THEN RETURN; END IF;
    PERFORM freight_project_tender_deadline(p_tenant,freight_canonical_selected(p_tenant,source_row.data,ARRAY['loads']),source_row.history_revision);
END $$;

CREATE FUNCTION freight_track_canonical_blocks() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE tenant text;
BEGIN
    IF TG_OP<>'INSERT' THEN
        UPDATE freight_block_counts SET records=records-jsonb_array_length(OLD.payload->'items'),blocks=blocks-1
        WHERE tenant_id=OLD.tenant_id AND root=OLD.root;
        IF NOT FOUND AND EXISTS(SELECT 1 FROM freight_workspaces WHERE tenant_id=OLD.tenant_id) THEN
            RAISE EXCEPTION 'Missing canonical block counter' USING ERRCODE='23514'; END IF;
        IF OLD.root='loads' THEN UPDATE freight_load_catalog SET valid=false WHERE tenant_id=OLD.tenant_id AND valid; END IF;
        UPDATE freight_workspaces SET inbox_due_at='-infinity',edi_due_at='-infinity',sync_due_at='-infinity' WHERE tenant_id=OLD.tenant_id;
    END IF;
    IF TG_OP<>'DELETE' THEN
        INSERT INTO freight_block_counts VALUES(NEW.tenant_id,NEW.root,jsonb_array_length(NEW.payload->'items'),1)
        ON CONFLICT(tenant_id,root) DO UPDATE SET records=freight_block_counts.records+EXCLUDED.records,blocks=freight_block_counts.blocks+1;
        IF NEW.root='loads' THEN UPDATE freight_load_catalog SET valid=false WHERE tenant_id=NEW.tenant_id AND valid; END IF;
        UPDATE freight_workspaces SET inbox_due_at='-infinity',edi_due_at='-infinity',sync_due_at='-infinity' WHERE tenant_id=NEW.tenant_id;
    END IF;
    RETURN NULL;
END $$;
CREATE FUNCTION freight_canonical_blocks_truncated() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE freight_block_counts SET records=0,blocks=0;
    UPDATE freight_load_catalog SET valid=false;
    UPDATE freight_workspaces SET inbox_due_at='-infinity',edi_due_at='-infinity',sync_due_at='-infinity';
    RETURN NULL;
END $$;

UPDATE freight_storage_format SET version=19 WHERE singleton;
DO $$ BEGIN
    IF EXISTS(SELECT 1 FROM freight_workspaces WHERE NOT coalesce(freight_canonical_header_valid(data),false))
       OR EXISTS(SELECT 1 FROM freight_workspaces w CROSS JOIN LATERAL jsonb_object_keys(w.data->'frame'->'roots') r
            WHERE NOT freight_canonical_root_valid(w.tenant_id,w.data,r)) THEN
        RAISE EXCEPTION 'Canonical backfill did not preserve complete metadata';
    END IF;
END $$;
-- Original business roots and revision values are unchanged by the backfill.
UPDATE freight_history_state SET source_format='19' WHERE source_format='18';
CREATE TRIGGER freight_storage_format_guard BEFORE INSERT OR UPDATE OF data ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION freight_require_storage_format();
CREATE TRIGGER freight_integration_wakeups BEFORE INSERT OR UPDATE OF data,inbox_due_at,edi_due_at,sync_due_at ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION freight_refresh_integration_wakeups();
CREATE TRIGGER freight_load_catalog_refresh AFTER INSERT OR UPDATE OF data ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION freight_refresh_load_catalog();
CREATE TRIGGER freight_history_revision_guard BEFORE UPDATE ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION freight_advance_history_revision();
CREATE CONSTRAINT TRIGGER freight_history_refresh AFTER INSERT OR UPDATE ON freight_workspaces DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION freight_refresh_history();
CREATE TRIGGER freight_tender_deadline_refresh AFTER INSERT OR UPDATE ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION freight_refresh_tender_deadline();
CREATE TRIGGER freight_block_writer_guard BEFORE INSERT OR UPDATE OR DELETE ON freight_workspace_blocks FOR EACH ROW EXECUTE FUNCTION freight_require_block_writer();
CREATE TRIGGER freight_block_changes AFTER INSERT OR UPDATE OR DELETE ON freight_workspace_blocks FOR EACH ROW EXECUTE FUNCTION freight_track_canonical_blocks();
CREATE TRIGGER freight_blocks_truncate AFTER TRUNCATE ON freight_workspace_blocks FOR EACH STATEMENT EXECUTE FUNCTION freight_canonical_blocks_truncated();
