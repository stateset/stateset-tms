-- Block writers deliberately invalidate hints before their final header update.
-- Reconstructing both queues for every block makes ingestion quadratic. An
-- all-due invalidation is conservative: workers still recheck canonical state
-- under the tenant lock. A header write always recomputes exact hints, including
-- UPDATE data=data after edits whose root cardinalities have not changed.
CREATE OR REPLACE FUNCTION freight_refresh_integration_wakeups() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE inbound jsonb; outbound jsonb;
BEGIN
    IF TG_NARGS=1 AND TG_ARGV[0]='invalidation'
       AND NEW.inbox_due_at='-infinity'::timestamptz
       AND NEW.edi_due_at='-infinity'::timestamptz
       AND NEW.sync_due_at='-infinity'::timestamptz THEN
        RETURN NEW;
    END IF;
    inbound:=freight_canonical_business_root(NEW.tenant_id,NEW.data,'inbox');
    outbound:=freight_canonical_root(NEW.tenant_id,NEW.data,'outbox');
    NEW.inbox_due_at:=CASE WHEN inbound IS NULL THEN '-infinity'::timestamptz ELSE freight_queue_due(inbound,true,NULL) END;
    NEW.edi_due_at:=CASE WHEN outbound IS NULL THEN '-infinity'::timestamptz ELSE freight_queue_due(outbound,false,'stateset-edi') END;
    NEW.sync_due_at:=CASE WHEN outbound IS NULL THEN '-infinity'::timestamptz ELSE freight_queue_due(outbound,false,'stateset-sync-server') END;
    RETURN NEW;
END $$;
DROP TRIGGER freight_integration_wakeups ON freight_workspaces;
CREATE TRIGGER freight_integration_wakeups
BEFORE INSERT OR UPDATE OF data ON freight_workspaces
FOR EACH ROW EXECUTE FUNCTION freight_refresh_integration_wakeups();
-- PostgreSQL runs these BEFORE triggers by name. On mixed data/hint writes the
-- exact header projection above runs after this conservative invalidation.
CREATE TRIGGER freight_integration_hint_invalidation
BEFORE UPDATE OF inbox_due_at,edi_due_at,sync_due_at ON freight_workspaces
FOR EACH ROW EXECUTE FUNCTION freight_refresh_integration_wakeups('invalidation');
