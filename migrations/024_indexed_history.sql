-- Rebuildable ordinal indexes. Original payloads remain exclusively in the
-- authoritative aggregate. A portable revision survives logical restoration.
ALTER TABLE freight_workspaces ADD COLUMN history_revision bigint NOT NULL DEFAULT 0 CHECK (history_revision>=0);
CREATE TABLE freight_history_state (
    tenant_id text PRIMARY KEY REFERENCES freight_workspaces(tenant_id) ON DELETE CASCADE,
    source_revision bigint NOT NULL, source_format text, loads_valid boolean NOT NULL,
    ready boolean NOT NULL DEFAULT false
);
CREATE TABLE freight_history_collections (
    tenant_id text NOT NULL REFERENCES freight_history_state(tenant_id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('events','appointments','stop_events')),
    valid boolean NOT NULL, source_length bigint NOT NULL CHECK (source_length>=0),
    expected_scopes bigint NOT NULL DEFAULT 0, actual_scopes bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant_id,kind)
);
CREATE TABLE freight_history_loads (
    tenant_id text NOT NULL REFERENCES freight_history_state(tenant_id) ON DELETE CASCADE,
    load_id text COLLATE "C" NOT NULL CHECK (octet_length(load_id)<=200),
    occurrences bigint NOT NULL CHECK (occurrences>0), version text,
    PRIMARY KEY (tenant_id,load_id)
);
CREATE FUNCTION freight_history_positions_valid(positions bigint[]) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT AS $$
    SELECT array_ndims(positions)=1 AND array_lower(positions,1)=1 AND cardinality(positions)>0
      AND NOT EXISTS (SELECT 1 FROM (
          SELECT p,lag(p) OVER(ORDER BY n) AS preceding FROM unnest(positions) WITH ORDINALITY v(p,n)
      ) ordered WHERE p IS NULL OR p<=0 OR p<=preceding);
$$;
CREATE TABLE freight_history_scopes (
    tenant_id text NOT NULL, kind text NOT NULL, scope_id text COLLATE "C" NOT NULL CHECK (octet_length(scope_id)<=200),
    positions bigint[] NOT NULL CHECK (freight_history_positions_valid(positions)),
    PRIMARY KEY (tenant_id,kind,scope_id),
    FOREIGN KEY (tenant_id,kind) REFERENCES freight_history_collections(tenant_id,kind) ON DELETE CASCADE
);
-- Statement counters detect missing scope rows without scanning every history
-- record at read time. Updating positions does not change scope cardinality.
CREATE FUNCTION freight_history_scopes_inserted() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE freight_history_collections c SET actual_scopes=c.actual_scopes+counts.n
    FROM (SELECT tenant_id,kind,count(*) n FROM new_scopes GROUP BY tenant_id,kind) counts
    WHERE c.tenant_id=counts.tenant_id AND c.kind=counts.kind;
    RETURN NULL;
END $$;
CREATE FUNCTION freight_history_scopes_deleted() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE freight_history_collections c SET actual_scopes=c.actual_scopes-counts.n
    FROM (SELECT tenant_id,kind,count(*) n FROM old_scopes GROUP BY tenant_id,kind) counts
    WHERE c.tenant_id=counts.tenant_id AND c.kind=counts.kind;
    RETURN NULL;
END $$;
CREATE TRIGGER freight_history_scopes_insert AFTER INSERT ON freight_history_scopes
REFERENCING NEW TABLE AS new_scopes FOR EACH STATEMENT EXECUTE FUNCTION freight_history_scopes_inserted();
CREATE TRIGGER freight_history_scopes_delete AFTER DELETE ON freight_history_scopes
REFERENCING OLD TABLE AS old_scopes FOR EACH STATEMENT EXECUTE FUNCTION freight_history_scopes_deleted();
CREATE FUNCTION freight_history_scopes_truncated() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN UPDATE freight_history_collections SET actual_scopes=0; RETURN NULL; END $$;
CREATE TRIGGER freight_history_scopes_truncate AFTER TRUNCATE ON freight_history_scopes
FOR EACH STATEMENT EXECUTE FUNCTION freight_history_scopes_truncated();

CREATE FUNCTION freight_advance_history_revision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN NEW.history_revision := OLD.history_revision+1; RETURN NEW; END $$;
CREATE TRIGGER freight_history_revision_guard BEFORE UPDATE ON freight_workspaces
FOR EACH ROW EXECUTE FUNCTION freight_advance_history_revision();

CREATE FUNCTION freight_project_history(p_tenant text,p_data jsonb,p_revision bigint)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE source_kind text; identity_field text; collection jsonb; loads jsonb:=p_data->'loads'; collection_valid boolean;
BEGIN
    INSERT INTO freight_history_state VALUES
        (p_tenant,p_revision,(p_data->'_freight_format')::text,coalesce(jsonb_typeof(loads)='array',false),true)
    ON CONFLICT (tenant_id) DO UPDATE SET source_revision=EXCLUDED.source_revision,
        source_format=EXCLUDED.source_format,loads_valid=EXCLUDED.loads_valid,ready=true;
        INSERT INTO freight_history_loads AS existing (tenant_id,load_id,occurrences,version)
        SELECT p_tenant,(l->>'id') COLLATE "C",count(*),max((l->'version')::text)
        FROM jsonb_array_elements(CASE WHEN jsonb_typeof(loads)='array' THEN loads ELSE '[]'::jsonb END) l
        WHERE l->>'id' IS NOT NULL AND octet_length(l->>'id')<=200 GROUP BY (l->>'id') COLLATE "C"
        ON CONFLICT (tenant_id,load_id) DO UPDATE SET occurrences=EXCLUDED.occurrences,version=EXCLUDED.version
        WHERE (existing.occurrences,existing.version) IS DISTINCT FROM (EXCLUDED.occurrences,EXCLUDED.version);
        DELETE FROM freight_history_loads existing WHERE existing.tenant_id=p_tenant AND NOT EXISTS (
            SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(loads)='array' THEN loads ELSE '[]'::jsonb END) l
            WHERE (l->>'id') COLLATE "C"=existing.load_id);
    FOREACH source_kind IN ARRAY ARRAY['events','appointments','stop_events'] LOOP
        collection:=coalesce(p_data->source_kind,'[]'::jsonb);
        identity_field:=CASE WHEN source_kind='events' THEN 'entity_id' ELSE 'load_id' END;
        collection_valid:=jsonb_typeof(collection)='array';
        IF collection_valid THEN
            SELECT NOT EXISTS (SELECT 1 FROM jsonb_array_elements(collection) item
                WHERE jsonb_typeof(item) IS DISTINCT FROM 'object'
                  OR jsonb_typeof(item->'id') IS DISTINCT FROM 'string' OR item->>'id'=''
                  OR jsonb_typeof(item->identity_field) IS DISTINCT FROM 'string') INTO collection_valid;
        END IF;
        INSERT INTO freight_history_collections AS existing (tenant_id,kind,valid,source_length)
        VALUES(p_tenant,source_kind,collection_valid,CASE WHEN jsonb_typeof(collection)='array' THEN jsonb_array_length(collection) ELSE 0 END)
        ON CONFLICT (tenant_id,kind) DO UPDATE SET valid=EXCLUDED.valid,source_length=EXCLUDED.source_length
        WHERE (existing.valid,existing.source_length) IS DISTINCT FROM (EXCLUDED.valid,EXCLUDED.source_length);
        IF collection_valid THEN
            WITH fresh AS MATERIALIZED (
                SELECT (item.value->>identity_field) COLLATE "C" AS scope_id,array_agg(item.ordinal ORDER BY item.ordinal) AS positions
                FROM jsonb_array_elements(collection) WITH ORDINALITY AS item(value,ordinal)
                WHERE octet_length(item.value->>identity_field)<=200 GROUP BY (item.value->>identity_field) COLLATE "C"
            ), inserted AS (
                INSERT INTO freight_history_scopes AS existing (tenant_id,kind,scope_id,positions)
                SELECT p_tenant,source_kind,scope_id,positions FROM fresh
                ON CONFLICT (tenant_id,kind,scope_id) DO UPDATE SET positions=EXCLUDED.positions
                WHERE existing.positions IS DISTINCT FROM EXCLUDED.positions RETURNING scope_id
            )
            DELETE FROM freight_history_scopes s WHERE s.tenant_id=p_tenant AND s.kind=source_kind
              AND NOT EXISTS (SELECT 1 FROM fresh WHERE fresh.scope_id COLLATE "C"=s.scope_id);
        ELSE
            DELETE FROM freight_history_scopes s WHERE s.tenant_id=p_tenant AND s.kind=source_kind;
        END IF;
        UPDATE freight_history_collections c SET expected_scopes=observed.n,actual_scopes=observed.n
        FROM (SELECT count(*) n FROM freight_history_scopes s WHERE s.tenant_id=p_tenant AND s.kind=source_kind) observed
        WHERE c.tenant_id=p_tenant AND c.kind=source_kind;
    END LOOP;
END $$;
CREATE FUNCTION freight_refresh_history() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM freight_workspaces w WHERE w.tenant_id=NEW.tenant_id AND w.history_revision=NEW.history_revision) THEN RETURN NEW; END IF;
    -- Invalidate in the source transaction without inspecting/detoasting its
    -- payload. Rebuild in a separate read statement after ingestion releases
    -- its temporary JSON parser memory. Invalidated ordinals are never served.
    INSERT INTO freight_history_state VALUES(NEW.tenant_id,NEW.history_revision,NULL,false,false)
    ON CONFLICT (tenant_id) DO UPDATE SET ready=false;
    RETURN NEW;
END $$;
-- Validity changes commit with the source. No background queue or stale-read
-- window is involved; a cold read builds before selecting its original page.
CREATE CONSTRAINT TRIGGER freight_history_refresh AFTER INSERT OR UPDATE ON freight_workspaces
DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION freight_refresh_history();
CREATE FUNCTION freight_rebuild_history(p_tenant text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source_row freight_workspaces%ROWTYPE;
BEGIN
    SELECT * INTO source_row FROM freight_workspaces WHERE tenant_id=p_tenant FOR UPDATE;
    IF FOUND THEN PERFORM freight_project_history(p_tenant,source_row.data,source_row.history_revision); END IF;
END $$;
CREATE FUNCTION freight_ensure_history(p_tenant text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source_row freight_workspaces%ROWTYPE;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM freight_workspaces w LEFT JOIN freight_history_state s USING(tenant_id)
        WHERE w.tenant_id=p_tenant AND (s.tenant_id IS NULL OR NOT s.ready OR s.source_revision<>w.history_revision)) THEN RETURN; END IF;
    SELECT * INTO source_row FROM freight_workspaces WHERE tenant_id=p_tenant FOR UPDATE;
    IF NOT FOUND THEN RETURN; END IF;
    -- A competing reader may have rebuilt while this one waited for the row.
    IF EXISTS (SELECT 1 FROM freight_history_state s WHERE s.tenant_id=p_tenant
        AND s.ready AND s.source_revision=source_row.history_revision) THEN RETURN; END IF;
    PERFORM freight_project_history(p_tenant,source_row.data,source_row.history_revision);
END $$;
-- No source UPDATE: preserve JSON, timestamps, versions and original receipts.
SELECT freight_project_history(tenant_id,data,history_revision) FROM freight_workspaces;
