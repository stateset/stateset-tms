-- psql runs this and its client-encrypted password command in one transaction.
SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '60s';
SET LOCAL password_encryption = 'scram-sha-256';
SELECT pg_advisory_xact_lock(785432019651);

DO $$
DECLARE
  runtime record;
  runtime_oid oid;
BEGIN
  IF current_schema() IS DISTINCT FROM 'public'
     OR to_regclass('public.freight_schema_migrations') IS NULL
     OR to_regclass('public.freight_storage_format') IS NULL THEN
    RAISE EXCEPTION 'Run the Freight schema job in the dedicated public schema first';
  END IF;
  IF (SELECT count(*) <> 31 OR min(version) <> 1 OR max(version) <> 31
      FROM public.freight_schema_migrations)
     OR (SELECT version FROM public.freight_storage_format WHERE singleton) IS DISTINCT FROM 21 THEN
    RAISE EXCEPTION 'Run the matching Freight schema job before configuring runtime grants';
  END IF;
  IF current_user = 'freight_app' THEN
    RAISE EXCEPTION 'Use the schema and role administrator';
  END IF;
  SELECT * INTO runtime FROM pg_roles WHERE rolname = 'freight_app';
  IF FOUND THEN
    runtime_oid := runtime.oid;
    IF shobj_description(runtime_oid, 'pg_authid') IS DISTINCT FROM 'stateset-freight managed runtime v1' THEN
      RAISE EXCEPTION 'Existing freight_app is not a managed Freight runtime role';
    END IF;
    IF runtime.rolsuper OR runtime.rolinherit OR runtime.rolcreaterole
       OR runtime.rolcreatedb OR NOT runtime.rolcanlogin
       OR runtime.rolreplication OR runtime.rolbypassrls
       OR runtime.rolvaliduntil IS NOT NULL
       OR runtime.rolconfig IS NOT NULL
       OR EXISTS(SELECT 1 FROM pg_db_role_setting WHERE setrole = runtime_oid) THEN
      RAISE EXCEPTION 'Managed runtime role has unexpected attributes or settings';
    END IF;
    IF EXISTS(SELECT 1 FROM pg_auth_members WHERE roleid = runtime_oid OR member = runtime_oid)
       OR EXISTS(SELECT 1 FROM pg_shdepend WHERE refclassid = 'pg_authid'::regclass
                 AND refobjid = runtime_oid AND deptype = 'o') THEN
      RAISE EXCEPTION 'Managed runtime role has memberships or owns database objects';
    END IF;
  ELSE
    CREATE ROLE freight_app LOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
    COMMENT ON ROLE freight_app IS 'stateset-freight managed runtime v1';
  END IF;
END $$;

DO $$
DECLARE
  item record;
BEGIN
  EXECUTE format('REVOKE ALL ON DATABASE %I FROM PUBLIC, freight_app', current_database());
  EXECUTE format('GRANT CONNECT ON DATABASE %I TO freight_app', current_database());
  REVOKE CREATE ON SCHEMA public FROM PUBLIC;
  REVOKE ALL ON SCHEMA public FROM freight_app;
  GRANT USAGE ON SCHEMA public TO freight_app;
  REVOKE ALL ON ALL TABLES IN SCHEMA public FROM freight_app;
  REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM freight_app;
  -- Table revocation does not revoke column grants. Remove those explicitly.
  FOR item IN
    SELECT c.relname, string_agg(format('%I', a.attname), ',' ORDER BY a.attnum) AS columns
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
    WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p')
    GROUP BY c.relname
  LOOP
    EXECUTE format('REVOKE ALL (%s) ON TABLE public.%I FROM freight_app', item.columns, item.relname);
  END LOOP;
  GRANT SELECT ON ALL TABLES IN SCHEMA public TO freight_app;
  GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO freight_app;
  FOR item IN
    SELECT tablename FROM pg_tables WHERE schemaname = 'public'
      AND tablename NOT IN ('freight_schema_migrations', 'freight_storage_format')
  LOOP
    EXECUTE format('GRANT INSERT, UPDATE, DELETE ON public.%I TO freight_app', item.tablename);
  END LOOP;
  GRANT UPDATE(singleton) ON public.freight_storage_format TO freight_app;
  IF has_schema_privilege('freight_app', 'public', 'CREATE')
     OR has_database_privilege('freight_app', current_database(), 'CREATE')
     OR has_database_privilege('freight_app', current_database(), 'TEMP')
     OR has_column_privilege('freight_app', 'public.freight_storage_format', 'version', 'UPDATE')
     OR has_table_privilege('freight_app', 'public.freight_schema_migrations', 'INSERT,UPDATE,DELETE,TRUNCATE,TRIGGER,REFERENCES') THEN
    RAISE EXCEPTION 'Runtime permissions remain elevated through another grant';
  END IF;
END $$;
