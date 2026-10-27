SELECT c.valid, p.summary::text FROM freight_load_catalog c
LEFT JOIN LATERAL (
    SELECT summary,load_id FROM freight_load_search s
    WHERE s.tenant_id=c.tenant_id AND s.load_id > $2 COLLATE "C"
      AND ($3::text IS NULL OR s.status=$3)
      AND ($4::text IS NULL OR s.customer_id=$4)
      AND ($5::text IS NULL OR s.carrier_id=$5)
      AND ($6='' OR strpos(s.search_text,$6) > 0)
    ORDER BY s.load_id LIMIT $7
) p ON c.valid WHERE c.tenant_id=$1 ORDER BY p.load_id
