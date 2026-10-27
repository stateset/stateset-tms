-- Only scalar source generation and derived deadline metadata cross this read.
-- Missing/stale metadata falls back to the original typed preflight.
SELECT d.tenant_id IS NULL OR NOT d.ready OR d.source_revision<>w.history_revision
    OR coalesce(d.due_at<=$2::text::timestamptz,false) AS may_be_due
FROM freight_workspaces w LEFT JOIN freight_tender_deadlines d USING(tenant_id)
WHERE w.tenant_id=$1
