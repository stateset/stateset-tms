//! Owned SQL fixtures only. Business API tests still exercise Store commands;
//! corruption/setup fixtures must use the actual canonical representation.
#![allow(dead_code)]
use serde::Serialize;
use serde_json::{value::RawValue, Value};
use sqlx::PgPool;

const FORMAT: i32 = 21;
use stateset_freight_storage_codec::{pack, Config};

/// Keep the historical minor-migration contract observable before the current
/// major representation upgrade fences those aggregate writers.
pub async fn minor_migration(pool: &PgPool, version: i32, sql: &str) -> Result<(), sqlx::Error> {
    use sha2::{Digest, Sha256};
    assert!(
        (1..=25).contains(&version) || (29..=31).contains(&version),
        "Native migrations 026–028 cannot run as SQL alone"
    );
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(sql).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO freight_schema_migrations(version,checksum) VALUES($1,$2)")
        .bind(version)
        .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

pub async fn put_workspace<T: Serialize>(
    pool: &PgPool,
    tenant: &str,
    workspace: &T,
) -> Result<(), sqlx::Error> {
    let mut value =
        serde_json::to_value(workspace).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    value
        .as_object_mut()
        .ok_or_else(|| sqlx::Error::Protocol("Fixture workspace must be an object".into()))?
        .insert("_freight_format".into(), Value::from(FORMAT));
    put(pool, tenant, &value).await
}

/// Edit an existing business field under the same tenant lock as the fixture
/// rewrite. Missing paths fail rather than silently making a corruption a no-op.
pub async fn set(
    pool: &PgPool,
    tenant: &str,
    path: &[&str],
    value: Value,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT version FROM freight_storage_format WHERE singleton FOR SHARE")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT tenant_id FROM freight_workspaces WHERE tenant_id=$1 FOR UPDATE")
        .bind(tenant)
        .execute(&mut *tx)
        .await?;
    let mut original: Value = sqlx::query_scalar("SELECT freight_workspace_originals($1)")
        .bind(tenant)
        .fetch_one(&mut *tx)
        .await?;
    let pointer = path
        .iter()
        .map(|s| format!("/{}", s.replace('~', "~0").replace('/', "~1")))
        .collect::<String>();
    *original
        .pointer_mut(&pointer)
        .ok_or_else(|| sqlx::Error::Protocol(format!("Missing fixture path {pointer}")))? = value;
    put_tx(&mut tx, tenant, &original).await?;
    tx.commit().await
}

pub async fn put(pool: &PgPool, tenant: &str, value: &Value) -> Result<(), sqlx::Error> {
    put_raw(
        pool,
        tenant,
        &serde_json::to_string(value).map_err(|e| sqlx::Error::Protocol(e.to_string()))?,
    )
    .await
}
pub async fn put_raw(pool: &PgPool, tenant: &str, raw: &str) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    put_raw_tx(&mut tx, tenant, raw).await?;
    tx.commit().await
}

pub async fn put_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    value: &Value,
) -> Result<(), sqlx::Error> {
    put_raw_tx(
        tx,
        tenant,
        &serde_json::to_string(value).map_err(|e| sqlx::Error::Protocol(e.to_string()))?,
    )
    .await
}

pub async fn put_raw_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    raw: &str,
) -> Result<(), sqlx::Error> {
    let mut frame = pack(
        raw,
        Config {
            span: 64,
            target_bytes: 65536,
        },
    )
    .map_err(sqlx::Error::Protocol)?;
    if frame
        .roots
        .get("_freight_format")
        .is_none_or(|root| root.scalar.get() != FORMAT.to_string())
    {
        return Err(sqlx::Error::Protocol(
            "Canonical fixture requires exact current format".into(),
        ));
    }
    let blocks = std::mem::take(&mut frame.blocks);
    #[derive(Serialize)]
    struct Header<'a> {
        #[serde(rename = "_freight_format")]
        format: i32,
        frame: &'a stateset_freight_storage_codec::Packed,
    }
    let header = serde_json::value::to_raw_value(&Header {
        format: FORMAT,
        frame: &frame,
    })
    .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    let version: i32 =
        sqlx::query_scalar("SELECT version FROM freight_storage_format WHERE singleton FOR SHARE")
            .fetch_one(&mut **tx)
            .await?;
    if version != FORMAT {
        return Err(sqlx::Error::Protocol(
            "Canonical fixture requires current registry".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO freight_workspaces(tenant_id,data) VALUES($1,$2) ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .bind(sqlx::types::Json(&header))
    .execute(&mut **tx)
    .await?;
    sqlx::query("SELECT tenant_id FROM freight_workspaces WHERE tenant_id=$1 FOR UPDATE")
        .bind(tenant)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM freight_workspace_blocks WHERE tenant_id=$1")
        .bind(tenant)
        .execute(&mut **tx)
        .await?;
    for chunk in blocks.chunks(64) {
        let roots = chunk.iter().map(|b| b.root.clone()).collect::<Vec<_>>();
        let keys = chunk.iter().map(|b| b.key.clone()).collect::<Vec<_>>();
        let bodies: Vec<sqlx::types::Json<Box<RawValue>>> = chunk
            .iter()
            .map(|b| serde_json::value::to_raw_value(b).map(sqlx::types::Json))
            .collect::<Result<_, _>>()
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        sqlx::query("INSERT INTO freight_workspace_blocks(tenant_id,root,key,payload) SELECT $1,root,key,payload FROM unnest($2::text[],$3::text[],$4::jsonb[]) v(root,key,payload)")
            .bind(tenant).bind(roots).bind(keys).bind(bodies).execute(&mut **tx).await?;
    }
    sqlx::query("UPDATE freight_workspaces SET data=$2 WHERE tenant_id=$1")
        .bind(tenant)
        .bind(sqlx::types::Json(header))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Negative fixture only: simulate stored marker damage, then reinstate the
/// writer guard before committing. Ordinary fixture writes never bypass it.
pub async fn corrupt_marker(pool: &PgPool, tenant: &str, marker: &str) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    corrupt_marker_tx(&mut tx, tenant, marker).await?;
    tx.commit().await
}

pub async fn corrupt_marker_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    marker: &str,
) -> Result<(), sqlx::Error> {
    let marker = RawValue::from_string(marker.to_owned())
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    sqlx::query("SELECT version FROM freight_storage_format WHERE singleton FOR SHARE")
        .execute(&mut **tx)
        .await?;
    sqlx::query("ALTER TABLE freight_workspaces DISABLE TRIGGER freight_storage_format_guard")
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE freight_workspaces SET data=jsonb_set(jsonb_set(data,'{_freight_format}',$2),'{frame,roots,_freight_format,scalar}',$2) WHERE tenant_id=$1")
        .bind(tenant).bind(sqlx::types::Json(marker)).execute(&mut **tx).await?;
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut **tx)
        .await?;
    sqlx::query("ALTER TABLE freight_workspaces ENABLE TRIGGER freight_storage_format_guard")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Observe every physical original and its membership metadata, not just the
/// header. Equal payloads with rewritten tuples must still fail this comparison.
pub async fn physical(pool: &PgPool, tenant: &str) -> Result<Value, sqlx::Error> {
    sqlx::query_scalar("SELECT jsonb_build_object('header',data,'xmin',xmin::text,'ctid',ctid::text,'updated_at',updated_at,'revision',history_revision,'blocks',(SELECT coalesce(jsonb_agg(jsonb_build_array(root,key,md5(payload::text),xmin::text,ctid::text) ORDER BY root COLLATE \"C\",key COLLATE \"C\"),'[]') FROM freight_workspace_blocks WHERE tenant_id=$1),'counts',(SELECT coalesce(jsonb_agg(jsonb_build_array(root,records,blocks,xmin::text,ctid::text) ORDER BY root COLLATE \"C\"),'[]') FROM freight_block_counts WHERE tenant_id=$1)) FROM freight_workspaces WHERE tenant_id=$1")
        .bind(tenant).fetch_one(pool).await
}

pub async fn source(pool: &PgPool, tenant: &str) -> Result<Value, sqlx::Error> {
    sqlx::query_scalar("SELECT freight_workspace_originals($1)")
        .bind(tenant)
        .fetch_one(pool)
        .await
}
