-- Preserve migration 021 and its deployed checksum. Validate the entire new
-- collection, but construct summaries and attempt upserts only for records
-- that differ by identity from a valid preceding source. No source rewrite,
-- backfill or additional stored business data is required.
CREATE FUNCTION freight_project_load_catalog(tenant text, workspace jsonb, previous jsonb)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE loads jsonb := workspace->'loads';
BEGIN
    -- Preserve malformed original data for diagnosis. Never represent a bad
    -- collection or ambiguous identity as a successful empty/partial catalog.
    IF jsonb_typeof(loads) IS DISTINCT FROM 'array' THEN
        INSERT INTO freight_load_catalog VALUES(tenant, false)
        ON CONFLICT (tenant_id) DO UPDATE SET valid=false;
        DELETE FROM freight_load_search WHERE tenant_id=tenant;
        RETURN;
    END IF;
    IF EXISTS (
        SELECT 1 FROM jsonb_array_elements(loads) AS item
        WHERE jsonb_typeof(item) IS DISTINCT FROM 'object'
           OR jsonb_typeof(item->'id') IS DISTINCT FROM 'string'
           OR item->>'id' = '' OR octet_length(item->>'id') > 200
           OR jsonb_typeof(item->'status') IS DISTINCT FROM 'string'
           OR item->>'status' NOT IN ('planned','tendered','accepted','dispatched','in_transit','delivered','cancelled')
           OR jsonb_typeof(item->'customer_id') IS DISTINCT FROM 'string'
           OR (item->'carrier_id' IS NOT NULL AND jsonb_typeof(item->'carrier_id') NOT IN ('string','null'))
    ) OR EXISTS (
        SELECT 1 FROM jsonb_array_elements(loads) AS item
        GROUP BY item->>'id' HAVING count(*) > 1
    ) THEN
        INSERT INTO freight_load_catalog VALUES(tenant, false)
        ON CONFLICT (tenant_id) DO UPDATE SET valid=false;
        DELETE FROM freight_load_search WHERE tenant_id=tenant;
        RETURN;
    END IF;
    -- Only a valid existing catalog can make unchanged raw records skippable.
    -- After invalidation the projection is empty, so repair must rebuild all
    -- rows even when most source records match the preceding array.
    IF jsonb_typeof(previous) IS DISTINCT FROM 'array' OR NOT EXISTS (
        SELECT 1 FROM freight_load_catalog WHERE tenant_id=tenant AND valid
    ) THEN
        previous := '[]'::jsonb;
    END IF;
    INSERT INTO freight_load_catalog VALUES(tenant, true)
    ON CONFLICT (tenant_id) DO UPDATE SET valid=true WHERE NOT freight_load_catalog.valid;
    INSERT INTO freight_load_search AS existing
        (tenant_id,load_id,status,customer_id,carrier_id,search_text,summary)
    SELECT tenant, item->>'id', item->>'status', item->>'customer_id', item->>'carrier_id',
        translate(concat_ws(' ',item->>'id',item->>'reference',item->>'order_number',
            item->>'location',item->'origin'->>'city',item->'origin'->>'state',
            item->'destination'->>'city',item->'destination'->>'state'),
            'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'),
        jsonb_build_object('id',item->'id','reference',item->'reference',
            'customer_id',item->'customer_id','carrier_id',item->'carrier_id',
            'status',item->'status','mode',item->'mode','equipment',item->'equipment',
            'location',item->'location','exception',item->'exception',
            'version',item->'version','updated_at',item->'updated_at',
            'origin',jsonb_build_object('city',item->'origin'->'city','state',item->'origin'->'state','date',item->'origin'->'date'),
            'destination',jsonb_build_object('city',item->'destination'->'city','state',item->'destination'->'state','date',item->'destination'->'date'))
    FROM (
        SELECT fresh.value AS item
        FROM jsonb_array_elements(loads) AS fresh(value)
        LEFT JOIN jsonb_array_elements(previous) AS old(value)
          ON (old.value->>'id') COLLATE "C" = (fresh.value->>'id') COLLATE "C"
        WHERE fresh.value IS DISTINCT FROM old.value
    ) AS changed
    ON CONFLICT (tenant_id,load_id) DO UPDATE SET
        status=EXCLUDED.status, customer_id=EXCLUDED.customer_id,
        carrier_id=EXCLUDED.carrier_id, search_text=EXCLUDED.search_text,
        summary=EXCLUDED.summary
    WHERE (existing.status,existing.customer_id,existing.carrier_id,existing.search_text,existing.summary)
        IS DISTINCT FROM (EXCLUDED.status,EXCLUDED.customer_id,EXCLUDED.carrier_id,EXCLUDED.search_text,EXCLUDED.summary);
    DELETE FROM freight_load_search AS existing
    WHERE existing.tenant_id=tenant AND NOT EXISTS (
        SELECT 1 FROM jsonb_array_elements(loads) AS item WHERE item->>'id'=existing.load_id
    );
END $$;
-- Keep the original rebuild entry point and its function identity. Explicit
-- rebuilds never use the incremental shortcut, including on older connections.
CREATE OR REPLACE FUNCTION freight_project_load_catalog(tenant text, workspace jsonb)
RETURNS void LANGUAGE sql AS $$
    SELECT freight_project_load_catalog(tenant,workspace,NULL::jsonb);
$$;

CREATE OR REPLACE FUNCTION freight_refresh_load_catalog() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE previous jsonb;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        previous := OLD.data->'loads';
        IF previous IS NOT DISTINCT FROM NEW.data->'loads' THEN RETURN NEW; END IF;
    END IF;
    PERFORM freight_project_load_catalog(NEW.tenant_id,NEW.data,previous);
    RETURN NEW;
END $$;
