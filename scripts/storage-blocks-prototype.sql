-- EXPERIMENT ONLY. Replace the trusted __LAYOUT__/__SPAN__ tokens in the owned
-- storage probe. This is not an application migration or a production schema.
CREATE TABLE __LAYOUT___heads (
    tenant_id text PRIMARY KEY,
    revision bigint NOT NULL DEFAULT 0,
    format integer NOT NULL CHECK(format=18),
    codec_version integer NOT NULL CHECK(codec_version=1),
    span integer NOT NULL CHECK(span=__SPAN__),
    target_bytes integer NOT NULL CHECK(target_bytes=65536),
    root_names text[] NOT NULL
);
CREATE TABLE __LAYOUT___roots (
    tenant_id text NOT NULL REFERENCES __LAYOUT___heads ON DELETE CASCADE,
    root text COLLATE "C" NOT NULL,
    descriptor jsonb NOT NULL CHECK(jsonb_typeof(descriptor)='object'),
    PRIMARY KEY(tenant_id,root)
);
CREATE TABLE __LAYOUT___blocks (
    tenant_id text NOT NULL,
    root text COLLATE "C" NOT NULL,
    key text COLLATE "C" NOT NULL,
    payload jsonb NOT NULL CHECK(jsonb_typeof(payload)='object'),
    PRIMARY KEY(tenant_id,root,key),
    FOREIGN KEY(tenant_id,root) REFERENCES __LAYOUT___roots ON DELETE CASCADE,
    CHECK(coalesce(payload->>'root'=root,false)),
    CHECK(coalesce(payload->>'key'=key,false)),
    CHECK(coalesce(jsonb_typeof(payload->'items')='array',false))
);
