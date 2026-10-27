-- EXPERIMENT ONLY. Not an application migration or a production schema.
-- Compare storage primitives using actual, already committed Freight outcomes.
-- The application still uses its format-18 aggregate and existing migrations.
CREATE TABLE probe_aggregates (
    tenant_id text PRIMARY KEY,
    revision bigint NOT NULL DEFAULT 0,
    data jsonb NOT NULL
);
CREATE TABLE probe_heads (
    tenant_id text PRIMARY KEY,
    revision bigint NOT NULL DEFAULT 0,
    format integer NOT NULL CHECK (format=18),
    root_names text[] NOT NULL
);
CREATE TABLE probe_roots (
    tenant_id text NOT NULL REFERENCES probe_heads ON DELETE CASCADE,
    root text COLLATE "C" NOT NULL,
    kind text NOT NULL CHECK (kind IN ('array','object','string','number','boolean','null')),
    size bigint NOT NULL CHECK (size>=0),
    scalar jsonb,
    PRIMARY KEY (tenant_id,root),
    CHECK (CASE WHEN kind IN ('array','object') THEN scalar IS NULL
        ELSE size=0 AND scalar IS NOT NULL AND jsonb_typeof(scalar)=kind END)
);
CREATE TABLE probe_items (
    tenant_id text NOT NULL,
    root text COLLATE "C" NOT NULL,
    position bigint NOT NULL CHECK (position>=0),
    name text COLLATE "C" NOT NULL,
    payload jsonb NOT NULL,
    PRIMARY KEY (tenant_id,root,position,name),
    FOREIGN KEY (tenant_id,root) REFERENCES probe_roots ON DELETE CASCADE
);
CREATE INDEX probe_item_identity ON probe_items
    (tenant_id,root,((payload->>'id') COLLATE "C"));

-- One-time backfill from the exact PostgreSQL JSONB source, preserving array
-- ordinals and literal object keys. This does not decode/re-encode business
-- models, deduplicate identities, or reinterpret unsupported nested records.
CREATE FUNCTION probe_backfill(p_tenant text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source jsonb;
BEGIN
    SELECT data INTO STRICT source FROM probe_aggregates WHERE tenant_id=p_tenant FOR UPDATE;
    IF source->'_freight_format' IS DISTINCT FROM '18'::jsonb THEN
        RAISE EXCEPTION 'Probe requires exact format 18';
    END IF;
    INSERT INTO probe_heads VALUES (p_tenant,0,18,
        (SELECT array_agg(key ORDER BY key COLLATE "C") FROM jsonb_each(source)));
    INSERT INTO probe_roots
    SELECT p_tenant,key,jsonb_typeof(value),
        CASE jsonb_typeof(value) WHEN 'array' THEN jsonb_array_length(value)
            WHEN 'object' THEN (SELECT count(*) FROM jsonb_object_keys(value)) ELSE 0 END,
        CASE WHEN jsonb_typeof(value) IN ('array','object') THEN NULL ELSE value END
    FROM jsonb_each(source);
    INSERT INTO probe_items
    SELECT p_tenant,root.key,item.ordinal,'',item.value
    FROM jsonb_each(source) root
    CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(root.value)='array'
        THEN root.value ELSE '[]'::jsonb END) WITH ORDINALITY item(value,ordinal)
    UNION ALL
    SELECT p_tenant,root.key,0,item.key,item.value
    FROM jsonb_each(source) root
    CROSS JOIN LATERAL jsonb_each(CASE WHEN jsonb_typeof(root.value)='object'
        THEN root.value ELSE '{}'::jsonb END) item;
END $$;
