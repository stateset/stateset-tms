"""Read canonical originals once per root, retaining the database validators.

This is measurement SQL, not a change to the serving application. SQL NULL
means invalid/missing data; JSON null must remain a legitimate root value.
"""

import re
import json


def original_query(quoted_tenant):
    if not re.fullmatch(r"'(?:[^']|'')*'", quoted_tenant):
        raise ValueError('Expected one escaped SQL text literal')
    return f"""/* equivalent freight_workspace_originals( observer query */
WITH observed_header AS MATERIALIZED (
    SELECT data FROM freight_workspaces
    WHERE tenant_id={quoted_tenant}
      AND coalesce(freight_canonical_header_valid(data),false)
), observed_roots AS MATERIALIZED (
    SELECT names.name,
           freight_canonical_root({quoted_tenant},h.data,names.name) AS value
    FROM observed_header h
    CROSS JOIN LATERAL jsonb_object_keys(h.data->'frame'->'roots') AS names(name)
)
SELECT (CASE
    WHEN (SELECT count(*) FROM observed_header)<>1 OR count(value)<>count(*)
        THEN NULL::jsonb
    ELSE coalesce(jsonb_object_agg(name,value),'{{}}'::jsonb)
END)::text AS originals FROM observed_roots"""


def substitute(statement):
    match = re.fullmatch(r"SELECT freight_workspace_originals\(('(?:[^']|'')*')\)::text", statement)
    return original_query(match[1]) if match else statement


def validation_sql():
    """Damage only transaction-local copies; serving fixture rows stay untouched."""
    cases = [
        ('original_probe', '', True, 'probe'),
        ('future_header', "UPDATE freight_workspaces SET data=jsonb_set(data,'{_freight_format}','999')", False, 'probe'),
        ('missing_block', "DELETE FROM freight_workspace_blocks WHERE ctid=(SELECT ctid FROM freight_workspace_blocks WHERE root='loads' LIMIT 1)", False, 'probe'),
        ('wrong_counter', "UPDATE freight_block_counts SET records=records+1 WHERE root='loads'", False, 'probe'),
        ('duplicate_position', "UPDATE freight_workspace_blocks SET payload=jsonb_set(payload,'{items,1,position}',payload#>'{items,0,position}') WHERE ctid=(SELECT ctid FROM freight_workspace_blocks WHERE root='loads' AND jsonb_array_length(payload->'items')>1 LIMIT 1)", False, 'probe'),
        ('absent_tenant', '', False, 'missing-owned-observer'),
    ]
    for name, kind, value in [
        ('scalar_json_null', 'null', 'null'),
        ('arbitrary_numeric_precision_and_scale', 'number', '12345678901234567890123456789.123456789000'),
        ('unicode_escaped_string', 'string', json.dumps('Zürich ♥\n"original"\\bytes', ensure_ascii=False)),
    ]:
        descriptor = '{"kind":"%s","size":0,"scalar":%s}' % (kind, value)
        update = "UPDATE freight_workspaces SET data=jsonb_set(data,'{frame,roots,observer_value}','" + descriptor.replace("'", "''") + "'::jsonb); "
        update += "UPDATE freight_workspaces SET data=jsonb_set(data,'{frame,root_names}',(SELECT jsonb_agg(key ORDER BY key COLLATE \"C\") FROM jsonb_object_keys(data#>'{frame,roots}') key))"
        cases.append((name, update, True, 'probe'))
    statements = [
        'BEGIN',
        "CREATE TEMP TABLE freight_workspaces AS SELECT tenant_id,data FROM public.freight_workspaces WHERE tenant_id='probe'",
        "CREATE TEMP TABLE freight_workspace_blocks AS SELECT * FROM public.freight_workspace_blocks WHERE tenant_id='probe'",
        "CREATE TEMP TABLE freight_block_counts AS SELECT * FROM public.freight_block_counts WHERE tenant_id='probe'",
    ]
    for name, update, valid, tenant in cases:
        statements.append('SAVEPOINT owned_case')
        if update:
            statements.append(update)
        statements.append("WITH legacy AS MATERIALIZED (SELECT freight_workspace_originals('%s')::text AS value), current AS MATERIALIZED (%s) SELECT jsonb_build_object('case','%s','legacy_valid',legacy.value IS NOT NULL,'exact_text_equal',legacy.value IS NOT DISTINCT FROM current.originals)::text FROM legacy CROSS JOIN current" % (tenant, original_query("'" + tenant + "'"), name))
        statements.extend(['ROLLBACK TO SAVEPOINT owned_case', 'RELEASE SAVEPOINT owned_case'])
    statements.append('ROLLBACK')
    return ';\n'.join(statements) + ';', {name: valid for name, _, valid, _ in cases}
