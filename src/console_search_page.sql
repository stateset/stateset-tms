-- Search every exposed value once. Keep only keys in the shared result;
-- fetch potentially toasted summaries after selecting the bounded page.
WITH args AS (SELECT $3::text AS balance,$4::text AS direction),
state AS MATERIALIZED (
    SELECT * FROM freight_console_state WHERE tenant_id=$1
),
matched AS MATERIALIZED (
    SELECT s.record_key
    FROM freight_console_records s JOIN state m ON s.tenant_id=m.tenant_id
    WHERE s.view=$2 COLLATE "C" AND m.ready AND m.as_of=$8::text::date /*FILTERS*/
)
SELECT m.ready AND m.as_of=$8::text::date AS ready,m.finance::text,m.errors::text,
       facts.total,facts.identities,facts.anchor,page.rows::text
FROM state m
LEFT JOIN LATERAL (
    SELECT count(*) AS total,
        encode(sha256(convert_to(coalesce(string_agg(encode(convert_to(s.record_key,'UTF8'),'hex'),':' ORDER BY s.record_key COLLATE "C"),''),'UTF8')),'hex') AS identities,
        coalesce(bool_or(s.record_key=$6 COLLATE "C"),false) AS anchor
    FROM matched s
) facts ON m.ready AND m.as_of=$8::text::date
LEFT JOIN LATERAL (
    SELECT coalesce(jsonb_agg(jsonb_build_object('key',p.record_key,'collection',r.collection,'record',r.summary) ORDER BY p.record_key COLLATE "C"),'[]'::jsonb) AS rows
    FROM (
        SELECT s.record_key FROM matched s
        WHERE s.record_key>$6 COLLATE "C"
        ORDER BY s.record_key COLLATE "C" LIMIT $7
    ) p
    JOIN LATERAL (
        SELECT r.collection,r.summary FROM freight_console_records r
        WHERE r.tenant_id=m.tenant_id AND r.view=$2 COLLATE "C" AND r.record_key=p.record_key
        -- Preserve a per-key lookup under generic as well as custom planning.
        OFFSET 0
    ) r ON true
) page ON m.ready AND m.as_of=$8::text::date
