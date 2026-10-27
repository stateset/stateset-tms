-- Scope lookup is a complete tenant/kind/id primary-key probe. Ordinal arrays
-- contain no copied payloads. Validate the root once, then read each requested
-- block group once for the page and its retained anchor. Only requested original
-- payloads are materialized; duplicate matches and out-of-range positions remain
-- invalid. Source revision and original identities fence stale indexes.
WITH head AS MATERIALIZED (
    SELECT w.tenant_id IS NOT NULL AS present,
        w.tenant_id IS NULL OR coalesce((w.data->'_freight_format')::text=$7::text
            AND (w.data->'frame'->'roots'->'_freight_format'->'scalar')::text=$7::text
            AND (s.tenant_id IS NULL OR s.source_format=$7::text),false) AS marker_valid,
        w.tenant_id IS NULL OR coalesce(s.ready AND s.source_revision=w.history_revision AND c.valid
            AND c.expected_scopes=c.actual_scopes AND c.expected_scopes>=0 AND c.actual_scopes>=0
            AND ($3::text IS NULL OR s.loads_valid),false) AS valid,
        coalesce(c.source_length,0) AS source_length,coalesce($4::bigint,c.source_length,0) AS upper_bound,
        CASE WHEN $3::text IS NULL THEN 1::bigint ELSE coalesce(l.occurrences,0) END AS load_count,
        l.version AS load_version,coalesce(scope.positions,ARRAY[]::bigint[]) AS positions
    FROM (SELECT 1) singleton
    LEFT JOIN freight_workspaces w ON w.tenant_id=$1
    LEFT JOIN freight_history_state s ON s.tenant_id=w.tenant_id
    LEFT JOIN freight_history_collections c ON c.tenant_id=s.tenant_id AND c.kind=$2
    LEFT JOIN freight_history_loads l ON l.tenant_id=s.tenant_id AND l.load_id=$3 COLLATE "C"
    LEFT JOIN freight_history_scopes scope ON scope.tenant_id=s.tenant_id AND scope.kind=$2 AND scope.scope_id=$3 COLLATE "C"
), metadata AS MATERIALIZED (
    SELECT head.*,
        CASE WHEN $3::text IS NULL THEN least(upper_bound,source_length)
            ELSE (SELECT count(*) FROM unnest(positions) p WHERE p<=upper_bound) END AS total,
        CASE WHEN $3::text IS NULL THEN source_length>upper_bound
            ELSE coalesce(positions[cardinality(positions)]>upper_bound,false) END AS newer
    FROM head
), page_positions AS MATERIALIZED (
    SELECT p.position FROM metadata
    CROSS JOIN LATERAL (
        SELECT pos AS position FROM unnest(positions) pos
        WHERE $3::text IS NOT NULL AND pos<=upper_bound AND pos<coalesce($5::bigint,upper_bound+1)
        UNION ALL
        SELECT pos FROM generate_series(least(upper_bound,source_length,coalesce($5::bigint-1,upper_bound)),
            greatest(1,least(upper_bound,source_length,coalesce($5::bigint-1,upper_bound))-$6::bigint+1),-1) pos
        WHERE $3::text IS NULL
    ) p ORDER BY p.position DESC LIMIT $6::bigint
), originals AS MATERIALIZED (
    SELECT w.data AS header,w.tenant_id IS NULL OR CASE
        WHEN $2::text IN ('appointments','stop_events') AND NOT w.data->'frame'->'roots' ? $2::text
        THEN coalesce(freight_canonical_business_root($1,w.data,$2)='[]'::jsonb AND metadata.source_length=0,false)
        ELSE (coalesce(freight_canonical_root_valid($1,w.data,$2),false)
        AND w.data->'frame'->'roots'->$2::text->>'kind'='array'
        AND (w.data->'frame'->'roots'->$2::text->>'size')::numeric=metadata.source_length) END AS source_valid
    FROM metadata LEFT JOIN freight_workspaces w ON w.tenant_id=$1
), requested AS MATERIALIZED (
    SELECT position FROM page_positions
    UNION
    SELECT upper_bound FROM metadata WHERE upper_bound>0 AND upper_bound<=source_length
), block_groups AS MATERIALIZED (
    SELECT DISTINCT 'a/'||lpad(to_hex((r.position-1)/64),16,'0')||'/' AS prefix
    FROM requested r CROSS JOIN metadata m CROSS JOIN originals o
    WHERE o.source_valid AND r.position>0 AND r.position<=m.source_length
), block_items AS MATERIALIZED (
    SELECT (item->'position')::text AS position,item->'payload' AS record
    FROM block_groups g
    CROSS JOIN LATERAL (
        SELECT b.payload FROM freight_workspace_blocks b
        WHERE b.tenant_id=$1 AND b.root=$2 COLLATE "C"
            AND b.key>=g.prefix COLLATE "C" AND b.key<(g.prefix||'~') COLLATE "C"
        -- Keep the per-group index range in generic prepared plans instead of
        -- pulling every block in the collection into a later join filter.
        OFFSET 0
    ) b
    CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') item
    WHERE item->'name'='""'::jsonb
        AND EXISTS (SELECT 1 FROM requested r CROSS JOIN metadata m
            WHERE r.position>0 AND r.position<=m.source_length AND (item->'position')::text=r.position::text)
), located AS MATERIALIZED (
    SELECT r.position,CASE WHEN count(i.position)=1 THEN jsonb_agg(i.record)->0 END AS record
    FROM requested r LEFT JOIN block_items i ON i.position=r.position::text
    GROUP BY r.position
), selected AS MATERIALIZED (
    SELECT p.position,l.record FROM page_positions p JOIN located l USING(position)
), verified AS MATERIALIZED (
    SELECT metadata.*,metadata.valid AND o.source_valid
        AND (cardinality(positions)=0 OR positions[cardinality(positions)]<=source_length)
        AND NOT EXISTS (SELECT 1 FROM selected p WHERE
            jsonb_typeof(p.record->'id') IS DISTINCT FROM 'string' OR p.record->>'id'=''
            OR jsonb_typeof(p.record->CASE WHEN $2::text='events' THEN 'entity_id' ELSE 'load_id' END) IS DISTINCT FROM 'string'
            OR ($3::text IS NOT NULL AND (p.record->>CASE WHEN $2::text='events' THEN 'entity_id' ELSE 'load_id' END) COLLATE "C" IS DISTINCT FROM $3)) AS verified_valid,
        CASE WHEN upper_bound>0 AND upper_bound<=source_length
            THEN encode(sha256(convert_to((SELECT record->>'id' FROM located WHERE position=upper_bound),'UTF8')),'hex') END AS anchor
    FROM metadata CROSS JOIN originals o
)
SELECT marker_valid,verified_valid AS valid,load_count,load_version,source_length,upper_bound,anchor,total,newer,
       page.position,page.record::text AS record
FROM verified LEFT JOIN selected page ON true ORDER BY page.position DESC
