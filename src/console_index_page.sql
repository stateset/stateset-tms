WITH args AS (SELECT $3::text AS balance,$4::text AS direction)
SELECT m.ready AND m.as_of=$8::text::date AS ready,m.finance::text,m.errors::text,
       facts.total,facts.identities,facts.anchor, page.rows::text
FROM freight_console_state m
LEFT JOIN LATERAL (
    SELECT count(*) AS total,
        encode(sha256(convert_to(coalesce(string_agg(encode(convert_to(s.record_key,'UTF8'),'hex'),':' ORDER BY s.record_key COLLATE "C"),''),'UTF8')),'hex') AS identities,
        coalesce(bool_or(s.record_key=$6 COLLATE "C"),false) AS anchor
    FROM freight_console_records s
    WHERE s.tenant_id=m.tenant_id AND s.view=$2 COLLATE "C" AND m.ready AND m.as_of=$8::text::date /*FILTERS*/
) facts ON m.ready AND m.as_of=$8::text::date
LEFT JOIN LATERAL (
    SELECT coalesce(jsonb_agg(jsonb_build_object('key',p.record_key,'collection',p.collection,'record',p.summary) ORDER BY p.record_key COLLATE "C"),'[]'::jsonb) AS rows
    FROM (
        SELECT s.record_key,s.collection,s.summary
        FROM freight_console_records s
        WHERE s.tenant_id=m.tenant_id AND s.view=$2 COLLATE "C" AND m.ready AND m.as_of=$8::text::date /*FILTERS*/
          AND s.record_key>$6 COLLATE "C"
        ORDER BY s.record_key LIMIT $7
    ) p
) page ON m.ready AND m.as_of=$8::text::date
WHERE m.tenant_id=$1
