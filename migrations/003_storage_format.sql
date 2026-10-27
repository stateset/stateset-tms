-- The whole tenant aggregate is rewritten on every mutation. A binary that
-- does not understand a newer aggregate must never serialize it back to disk.
CREATE TABLE IF NOT EXISTS freight_storage_format (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    version integer NOT NULL CHECK (version > 0)
);
INSERT INTO freight_storage_format(singleton, version)
VALUES (true, 1) ON CONFLICT DO NOTHING;

DO $$
BEGIN
    -- Lock order: format row, then tenant rows. Writers use the same order.
    PERFORM 1 FROM freight_storage_format WHERE singleton AND version = 1 FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'Freight storage format is incompatible with this API; deploy a compatible release';
    END IF;
END $$;

LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE;
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM freight_workspaces
        WHERE jsonb_typeof(data) IS DISTINCT FROM 'object'
           OR (data ? '_freight_format' AND data->'_freight_format' IS DISTINCT FROM '1'::jsonb)
    ) THEN
        RAISE EXCEPTION 'Freight workspace has an unsupported storage format; migration refused';
    END IF;
END $$;

-- Preserve the exact existing business data, including unknown extension keys.
UPDATE freight_workspaces
SET data = data || '{"_freight_format":1}'::jsonb
WHERE NOT data ? '_freight_format';

CREATE OR REPLACE FUNCTION freight_require_storage_format() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE current_version integer;
BEGIN
    SELECT version INTO current_version FROM freight_storage_format
    WHERE singleton;
    IF current_version IS NULL
       OR jsonb_typeof(NEW.data) IS DISTINCT FROM 'object'
       OR NEW.data->'_freight_format' IS DISTINCT FROM to_jsonb(current_version) THEN
        RAISE EXCEPTION 'Incompatible freight writer; workspace update refused'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS freight_storage_format_guard ON freight_workspaces;
CREATE TRIGGER freight_storage_format_guard
BEFORE INSERT OR UPDATE OF data ON freight_workspaces
FOR EACH ROW EXECUTE FUNCTION freight_require_storage_format();
