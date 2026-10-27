-- One snapshot observes metadata, actual block cardinality and the selected
-- literal-key SHA-256 bucket. No unrelated tenant collection is reconstructed.
SELECT coalesce((data->'_freight_format')::text=$3::text AND (data->'frame'->'version')::text='1',false) AS marker_valid,
       coalesce(freight_canonical_root_valid(tenant_id,data,'receipts')
           AND data->'frame'->'roots'->'receipts'->>'kind'='object',false) AS receipts_valid,
       freight_canonical_receipt(tenant_id,data,$2::text)::text AS receipt
FROM freight_workspaces WHERE tenant_id=$1
