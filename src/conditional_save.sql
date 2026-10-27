-- Keep the original receipt and its format/collection guards at one snapshot.
-- A hit never executes the locking subplan. A miss locks the latest row; a
-- concurrent winner's receipt is rechecked by the complete typed mutation.
WITH original AS MATERIALIZED (
    SELECT coalesce((data->'_freight_format')::text=$3::text AND (data->'frame'->'version')::text='1',false) AS marker_valid,
           coalesce(freight_canonical_root_valid(tenant_id,data,'receipts')
               AND data->'frame'->'roots'->'receipts'->>'kind'='object',false) AS receipts_valid,
           freight_canonical_receipt(tenant_id,data,$2::text) AS receipt
    FROM freight_workspaces WHERE tenant_id=$1
)
SELECT marker_valid,receipts_valid,receipt::text,
       CASE WHEN receipt IS NULL THEN
           (SELECT data::text FROM freight_workspaces WHERE tenant_id=$1 FOR UPDATE)
       ELSE NULL::text END AS workspace
FROM original
