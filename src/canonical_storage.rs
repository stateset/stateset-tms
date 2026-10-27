//! Canonical block storage. The tenant row contains only frame metadata;
//! original collection members live exclusively in the block relation.
use crate::database_budget::error as database_error;
use crate::domain::Workspace;
use crate::store::INCOMPATIBLE_STORAGE;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgArguments, Arguments, PgConnection, Postgres, Transaction};
use stateset_freight_storage_codec::{self as codec, Block, Config, Packed};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

pub(crate) const FORMAT: i32 = 21;
const CONFIG: Config = Config {
    span: 64,
    target_bytes: 65_536,
};
pub(crate) const APPROVAL_ROOTS: [&str; 2] = ["events", "invoices"];
pub(crate) const APPROVAL_BLOCKS: &str = "SELECT root,key,payload::text FROM freight_workspace_blocks WHERE tenant_id=$1 AND root=ANY($2::text[]) ORDER BY root,key";
const BACKFILL: &str = "-- FREIGHT_CANONICAL_BACKFILL";
pub(crate) const UPSERT: &str = "INSERT INTO freight_workspace_blocks(tenant_id,root,key,payload) SELECT $1,root,key,payload FROM unnest($2::text[],$3::text[],$4::jsonb[]) AS items(root,key,payload) ON CONFLICT(tenant_id,root,key) DO UPDATE SET payload=EXCLUDED.payload";

#[path = "canonical_read_cache.rs"]
mod read_cache;
pub(crate) use read_cache::ReadCache;

fn invalid(_: impl std::fmt::Display) -> String {
    "Internal storage error".into()
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Header {
    #[serde(rename = "_freight_format")]
    format: i32,
    frame: Packed,
}
pub(crate) struct Snapshot {
    header: String,
    rows: Vec<(String, String, String)>,
}
/// A read-only projection, never a complete workspace or reusable write frame.
pub(crate) struct ApprovalSnapshot(Snapshot);
impl ApprovalSnapshot {
    pub(crate) fn volume(&self) -> (usize, usize, usize) {
        (
            self.0.header.len(),
            self.0.rows.len(),
            self.0.rows.iter().map(|(_, _, raw)| raw.len()).sum(),
        )
    }
    pub(crate) fn decode(self) -> Result<crate::invoice_review::ApprovalSource, String> {
        let mut frame = self.0.frame(FORMAT)?;
        // Check the full manifest and current model's root names/kinds before
        // making a smaller read view. Missing mandatory roots and unknown
        // business roots remain failures; absent optional roots stay optional.
        if frame.root_names != frame.roots.keys().cloned().collect::<Vec<_>>() {
            return Err(invalid("Invalid root manifest"));
        }
        let mut defaults = serde_json::to_value(Workspace::default()).map_err(invalid)?;
        let shape = defaults
            .as_object_mut()
            .ok_or_else(|| invalid("Invalid model shape"))?;
        for (name, root) in &frame.roots {
            if name == "_freight_format" {
                continue;
            }
            let expected = shape
                .get(name)
                .ok_or_else(|| invalid("Unsupported business root"))?;
            let compatible = match name.as_str() {
                "operations_cases" | "operations_policy" => true,
                "billing_profile" => matches!(root.kind, codec::Kind::Null | codec::Kind::Object),
                _ if expected.is_array() => root.kind == codec::Kind::Array,
                _ if expected.is_object() => root.kind == codec::Kind::Object,
                _ => false,
            };
            if !compatible {
                return Err(invalid("Invalid business root kind"));
            }
        }
        shape.retain(|name, _| frame.roots.contains_key(name));
        serde_json::from_value::<Workspace>(defaults).map_err(invalid)?;
        for block in &frame.blocks {
            if !APPROVAL_ROOTS.contains(&block.root.as_str()) {
                return Err(invalid("Unexpected original in approval read"));
            }
        }
        for (name, root) in &mut frame.roots {
            if !APPROVAL_ROOTS.contains(&name.as_str())
                && matches!(root.kind, codec::Kind::Array | codec::Kind::Object)
            {
                root.size = 0;
            }
        }
        // The ordinary codec checks complete invoice/event membership,
        // ordinals, duplicates, partitions, opaque values and numeric budgets.
        // Unread collection payloads are not certified by this scoped read.
        let raw = codec::unpack(frame).map_err(invalid)?;
        serde_json::from_str(&raw).map_err(invalid)
    }
}
pub(crate) async fn approval_snapshot(
    connection: &mut PgConnection,
    tenant: &str,
) -> Result<Option<ApprovalSnapshot>, String> {
    let header: Option<String> =
        sqlx::query_scalar("SELECT data::text FROM freight_workspaces WHERE tenant_id=$1")
            .bind(tenant)
            .fetch_optional(&mut *connection)
            .await
            .map_err(database_error)?;
    match header {
        None => Ok(None),
        Some(header) => {
            let rows = sqlx::query_as(APPROVAL_BLOCKS)
                .bind(tenant)
                .bind(APPROVAL_ROOTS.as_slice())
                .fetch_all(connection)
                .await
                .map_err(database_error)?;
            Ok(Some(ApprovalSnapshot(Snapshot { header, rows })))
        }
    }
}
impl Snapshot {
    fn frame(self, expected_format: i32) -> Result<Packed, String> {
        crate::read_json::check_format(&self.header, expected_format).map_err(|e| match e {
            crate::read_json::DecodeError::Incompatible => INCOMPATIBLE_STORAGE.into(),
            crate::read_json::DecodeError::Invalid(e) => invalid(e),
        })?;
        let mut header: Header = serde_json::from_str(&self.header).map_err(invalid)?;
        if header.format != expected_format
            || header.frame.version != codec::VERSION
            || header.frame.config != CONFIG
        {
            return Err(INCOMPATIBLE_STORAGE.into());
        }
        if !header.frame.blocks.is_empty() {
            return Err(invalid("Inline originals are forbidden"));
        }
        if header
            .frame
            .roots
            .get("_freight_format")
            .is_none_or(|marker| {
                marker.kind != codec::Kind::Number
                    || marker.size != 0
                    || marker.scalar.get() != expected_format.to_string()
            })
        {
            return Err(INCOMPATIBLE_STORAGE.into());
        }
        for (root, key, raw) in self.rows {
            let block: Block = serde_json::from_str(&raw).map_err(invalid)?;
            if block.root != root || block.key != key {
                return Err(invalid("Block envelope mismatch"));
            }
            header.frame.blocks.push(block);
        }
        Ok(header.frame)
    }
    fn materialize(self) -> Result<(Workspace, String, codec::CanonicalFrame), String> {
        let (source, frame) = codec::unpack_for_reuse(self.frame(FORMAT)?).map_err(invalid)?;
        let workspace = crate::read_json::decode(&source, FORMAT).map_err(|e| match e {
            crate::read_json::DecodeError::Incompatible => INCOMPATIBLE_STORAGE.into(),
            crate::read_json::DecodeError::Invalid(e) => invalid(e),
        })?;
        Ok((workspace, source, frame))
    }
    pub(crate) fn decode(self) -> Result<Workspace, String> {
        // Ordinary reads need the same complete codec validation, but do not
        // retain a reusable write frame while constructing the typed model.
        // unpack consumes and releases that frame before typed decoding.
        let source = codec::unpack(self.frame(FORMAT)?).map_err(invalid)?;
        crate::read_json::decode(&source, FORMAT).map_err(|e| match e {
            crate::read_json::DecodeError::Incompatible => INCOMPATIBLE_STORAGE.into(),
            crate::read_json::DecodeError::Invalid(e) => invalid(e),
        })
    }
    pub(crate) fn decode_cached(
        self,
        tenant: &str,
        cache: &ReadCache,
    ) -> Result<Workspace, String> {
        // The transaction still fetches every current header/block byte. No
        // row-version, timer or caller hint may stand in for this exact digest.
        let Some(key) = read_cache::key(&self, tenant) else {
            return self.decode();
        };
        if let Some(source) = cache.get(key) {
            // Release the freshly hashed SQL text before allocating the typed
            // workspace, just as the consuming complete codec path does.
            drop(self);
            return decode_source(&source);
        }
        let source = codec::unpack(self.frame(FORMAT)?).map_err(invalid)?;
        let workspace = decode_source(&source)?;
        // Publish only after BOTH complete frame and typed-model validation.
        cache.insert(key, &source);
        Ok(workspace)
    }
    pub(crate) fn decode_mutation(self) -> Result<(Workspace, Baseline), String> {
        let (workspace, source, frame) = self.materialize()?;
        let projected = crate::write_json::encode(&workspace, FORMAT).map_err(invalid)?;
        let mut blocks = BTreeMap::<String, BTreeMap<String, [u8; 32]>>::new();
        for block in frame.blocks() {
            blocks
                .entry(block.root.clone())
                .or_default()
                .insert(block.key.clone(), block_digest(block)?);
        }
        let original = RawValue::from_string(source).map_err(invalid)?;
        Ok((
            workspace,
            Baseline {
                original,
                projected,
                blocks,
                frame,
            },
        ))
    }
}
fn decode_source(source: &str) -> Result<Workspace, String> {
    crate::read_json::decode(source, FORMAT).map_err(|e| match e {
        crate::read_json::DecodeError::Incompatible => INCOMPATIBLE_STORAGE.into(),
        crate::read_json::DecodeError::Invalid(e) => invalid(e),
    })
}
pub(crate) struct Baseline {
    original: Box<RawValue>,
    projected: Box<RawValue>,
    blocks: BTreeMap<String, BTreeMap<String, [u8; 32]>>,
    frame: codec::CanonicalFrame,
}
pub(crate) async fn snapshot(
    connection: &mut PgConnection,
    tenant: &str,
    lock: bool,
) -> Result<Option<Snapshot>, String> {
    let sql = if lock {
        "SELECT data::text FROM freight_workspaces WHERE tenant_id=$1 FOR UPDATE"
    } else {
        "SELECT data::text FROM freight_workspaces WHERE tenant_id=$1"
    };
    let header: Option<String> = sqlx::query_scalar(sql)
        .bind(tenant)
        .fetch_optional(&mut *connection)
        .await
        .map_err(database_error)?;
    match header {
        Some(header) => snapshot_from_header(connection, tenant, header)
            .await
            .map(Some),
        None => Ok(None),
    }
}
pub(crate) async fn snapshot_from_header(
    connection: &mut PgConnection,
    tenant: &str,
    header: String,
) -> Result<Snapshot, String> {
    let rows = sqlx::query_as("SELECT root,key,payload::text FROM freight_workspace_blocks WHERE tenant_id=$1 ORDER BY root,key")
        .bind(tenant).fetch_all(connection).await.map_err(database_error)?;
    Ok(Snapshot { header, rows })
}
pub(crate) fn encode(workspace: &Workspace) -> Result<Packed, String> {
    let raw = crate::write_json::encode(workspace, FORMAT).map_err(invalid)?;
    codec::pack(raw.get(), CONFIG).map_err(invalid)
}
fn head_at(mut frame: Packed, format: i32) -> Result<Box<RawValue>, String> {
    frame.blocks.clear();
    serde_json::value::to_raw_value(&Header { format, frame }).map_err(invalid)
}
fn head(frame: Packed) -> Result<Box<RawValue>, String> {
    head_at(frame, FORMAT)
}
pub(crate) async fn insert_empty(
    connection: &mut PgConnection,
    tenant: &str,
) -> Result<bool, String> {
    let mut frame = encode(&Workspace::default())?;
    let blocks = std::mem::take(&mut frame.blocks);
    let raw = head(frame)?;
    let inserted = sqlx::query(
        "INSERT INTO freight_workspaces(tenant_id,data) VALUES($1,$2) ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .bind(sqlx::types::Json(raw))
    .execute(&mut *connection)
    .await
    .map_err(database_error)?
    .rows_affected()
        == 1;
    if inserted {
        for chunk in blocks.chunks(64) {
            sqlx::query_with(UPSERT, block_arguments(tenant, chunk)?)
                .execute(&mut *connection)
                .await
                .map_err(database_error)?;
        }
    }
    Ok(inserted)
}
pub(crate) struct Prepared {
    upserts: Vec<PgArguments>,
    deleted: Option<PgArguments>,
    header: PgArguments,
}
fn block_digest(block: &Block) -> Result<[u8; 32], String> {
    Ok(Sha256::digest(serde_json::to_vec(block).map_err(invalid)?).into())
}
fn block_arguments(tenant: &str, blocks: &[Block]) -> Result<PgArguments, String> {
    let mut args = PgArguments::default();
    args.add(tenant.to_owned()).map_err(invalid)?;
    args.add(blocks.iter().map(|b| b.root.clone()).collect::<Vec<_>>())
        .map_err(invalid)?;
    args.add(blocks.iter().map(|b| b.key.clone()).collect::<Vec<_>>())
        .map_err(invalid)?;
    let bodies = blocks
        .iter()
        .map(|b| serde_json::value::to_raw_value(b).map(sqlx::types::Json))
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid)?;
    args.add(bodies).map_err(invalid)?;
    Ok(args)
}
pub(crate) fn prepare(
    tenant: &str,
    before: Baseline,
    workspace: &Workspace,
) -> Result<Prepared, String> {
    let profile = crate::write_profile::WriteProfile::new(tenant);
    let started = Instant::now();
    let Baseline {
        original,
        projected,
        blocks,
        frame,
    } = before;
    let mut after = {
        let encode_started = Instant::now();
        let fresh = crate::write_json::encode(workspace, FORMAT).map_err(invalid)?;
        profile.stage("encode", encode_started);
        let retain_started = Instant::now();
        let retained =
            crate::retained_json::merge(&original, &projected, &fresh).map_err(invalid)?;
        profile.stage("retain", retain_started);
        let pack_started = Instant::now();
        let packed = codec::pack_reusing(retained.get(), CONFIG, frame).map_err(invalid)?;
        profile.stage("pack", pack_started);
        packed
    };
    let delta_started = Instant::now();
    drop(original);
    drop(projected);
    let current: BTreeSet<_> = after
        .blocks
        .iter()
        .map(|b| (b.root.as_str(), b.key.as_str()))
        .collect();
    let removed: Vec<_> = blocks
        .iter()
        .flat_map(|(root, keys)| keys.keys().map(move |key| (root, key)))
        .filter(|(root, key)| !current.contains(&(root.as_str(), key.as_str())))
        .collect();
    let deleted = if removed.is_empty() {
        None
    } else {
        let mut args = PgArguments::default();
        args.add(tenant.to_owned()).map_err(invalid)?;
        args.add(
            removed
                .iter()
                .map(|(root, _)| (*root).to_owned())
                .collect::<Vec<_>>(),
        )
        .map_err(invalid)?;
        args.add(
            removed
                .iter()
                .map(|(_, key)| (*key).to_owned())
                .collect::<Vec<_>>(),
        )
        .map_err(invalid)?;
        Some(args)
    };
    drop(current);
    let mut changed = Vec::new();
    for block in std::mem::take(&mut after.blocks) {
        let digest = block_digest(&block)?;
        let old = blocks
            .get(&block.root)
            .and_then(|group| group.get(&block.key));
        if old != Some(&digest) {
            changed.push(block);
        }
    }
    let upserts = changed
        .chunks(64)
        .map(|b| block_arguments(tenant, b))
        .collect::<Result<Vec<_>, _>>()?;
    let mut header = PgArguments::default();
    header.add(tenant.to_owned()).map_err(invalid)?;
    header
        .add(sqlx::types::Json(head(after)?))
        .map_err(invalid)?;
    let prepared = Prepared {
        upserts,
        deleted,
        header,
    };
    profile.stage("delta", delta_started);
    profile.stage("complete", started);
    Ok(prepared)
}
pub(crate) async fn persist(
    connection: &mut PgConnection,
    prepared: Prepared,
) -> Result<(), String> {
    if let Some(args) = prepared.deleted {
        sqlx::query_with("DELETE FROM freight_workspace_blocks b USING unnest($2::text[],$3::text[]) AS removed(root,key) WHERE b.tenant_id=$1 AND b.root=removed.root AND b.key=removed.key",args)
            .execute(&mut *connection).await.map_err(database_error)?;
    }
    for args in prepared.upserts {
        sqlx::query_with(UPSERT, args)
            .execute(&mut *connection)
            .await
            .map_err(database_error)?;
    }
    sqlx::query_with(
        "UPDATE freight_workspaces SET data=$2,updated_at=now() WHERE tenant_id=$1",
        prepared.header,
    )
    .execute(connection)
    .await
    .map_err(database_error)?;
    Ok(())
}
pub(crate) async fn migrate(
    mut tx: Transaction<'static, Postgres>,
    sql: &str,
) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let (setup, finish) = sql
        .split_once(BACKFILL)
        .ok_or_else(|| sqlx::Error::Protocol("Missing canonical backfill boundary".into()))?;
    sqlx::raw_sql(setup).execute(&mut *tx).await?;
    let tenants: Vec<String> =
        sqlx::query_scalar("SELECT tenant_id FROM freight_workspaces ORDER BY tenant_id")
            .fetch_all(&mut *tx)
            .await?;
    for tenant in tenants {
        let raw: String = sqlx::query_scalar(
            "SELECT data::text FROM freight_workspaces WHERE tenant_id=$1 FOR UPDATE",
        )
        .bind(&tenant)
        .fetch_one(&mut *tx)
        .await?;
        let owner = tenant.clone();
        let (returned, pending, header) = tokio::task::spawn_blocking(move || {
            let mut frame = codec::pack(&raw, CONFIG).map_err(sqlx::Error::Protocol)?;
            let marker = frame
                .roots
                .get_mut("_freight_format")
                .ok_or_else(|| sqlx::Error::Protocol("Missing original format marker".into()))?;
            // Preserve unknown/malformed business roots for diagnosis. Typed
            // API reads still refuse them; migration never reserializes them.
            if marker.kind != codec::Kind::Number || marker.size != 0 || marker.scalar.get() != "18"
            {
                return Err(sqlx::Error::Protocol(
                    "Canonical migration requires exact format-18 source".into(),
                ));
            }
            // Historical migration026 produces format19. Migration027 owns
            // the separate policy fence, including full native validation.
            marker.scalar = RawValue::from_string("19".into())
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
            let pending = frame
                .blocks
                .chunks(64)
                .map(|blocks| block_arguments(&owner, blocks).map_err(sqlx::Error::Protocol))
                .collect::<Result<Vec<_>, _>>()?;
            let header = head_at(frame, 19).map_err(sqlx::Error::Protocol)?;
            Ok::<_, sqlx::Error>((tx, pending, header))
        })
        .await
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))??;
        tx = returned;
        for args in pending {
            sqlx::query_with(UPSERT, args).execute(&mut *tx).await?;
        }
        // Migration guards/maintenance are replaced atomically by the SQL
        // halves. Preserve timestamps and the portable revision on backfill.
        sqlx::query("UPDATE freight_workspaces SET data=$2 WHERE tenant_id=$1")
            .bind(&tenant)
            .bind(sqlx::types::Json(header))
            .execute(&mut *tx)
            .await?;
    }
    sqlx::raw_sql(finish).execute(&mut *tx).await?;
    Ok(tx)
}

pub(crate) async fn migrate_policy_fence(
    mut tx: Transaction<'static, Postgres>,
    sql: &str,
    previous_format: i32,
) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let (setup, finish) = sql
        .split_once("-- FREIGHT_POLICY_VALIDATION")
        .ok_or_else(|| {
            sqlx::Error::Protocol("Missing finance policy validation boundary".into())
        })?;
    sqlx::raw_sql(setup).execute(&mut *tx).await?;
    let tenants: Vec<String> =
        sqlx::query_scalar("SELECT tenant_id FROM freight_workspaces ORDER BY tenant_id")
            .fetch_all(&mut *tx)
            .await?;
    for tenant in tenants {
        let snapshot = snapshot(&mut tx, &tenant, false)
            .await
            .map_err(sqlx::Error::Protocol)?
            .ok_or_else(|| sqlx::Error::Protocol("Missing canonical tenant".into()))?;
        // The transaction owns the format lock through cancellation and CPU
        // validation. Unknown business extensions remain opaque; every frame
        // envelope, original, ordinal, key, number and partition still validates.
        tx = tokio::task::spawn_blocking(move || {
            let frame = snapshot
                .frame(previous_format)
                .map_err(sqlx::Error::Protocol)?;
            codec::unpack(frame).map_err(sqlx::Error::Protocol)?;
            Ok::<_, sqlx::Error>(tx)
        })
        .await
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))??;
    }
    sqlx::raw_sql(finish).execute(&mut *tx).await?;
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stored(mut frame: Packed) -> Snapshot {
        let rows = std::mem::take(&mut frame.blocks)
            .into_iter()
            .map(|block| {
                let raw = serde_json::to_string(&block).expect("block");
                (block.root, block.key, raw)
            })
            .collect();
        Snapshot {
            header: serde_json::to_string(&Header {
                format: FORMAT,
                frame,
            })
            .expect("header"),
            rows,
        }
    }
    fn approval_stored(frame: Packed) -> ApprovalSnapshot {
        let mut snapshot = stored(frame);
        snapshot
            .rows
            .retain(|(root, _, _)| APPROVAL_ROOTS.contains(&root.as_str()));
        ApprovalSnapshot(snapshot)
    }
    #[test]
    fn read_only_decoder_preserves_complete_mutation_decode_content() {
        let mut workspace = crate::domain::demo();
        workspace.loads[0].source_order = Some(serde_json::json!({
            "$serde_json::private::Number":"literal original", "unicode":"Zürich ♥",
            "nested":[null, true, {"reference":"quoted \"original\"\\path"}]
        }));
        let frame = encode(&workspace).expect("complete frame");
        let read = stored(encode(&workspace).expect("independent read frame"))
            .decode()
            .expect("read-only model");
        let (mutation, _) = stored(frame)
            .decode_mutation()
            .expect("write baseline model");
        assert_eq!(
            serde_json::to_value(read).expect("read"),
            serde_json::to_value(&workspace).expect("original")
        );
        assert_eq!(
            serde_json::to_value(mutation).expect("mutation"),
            serde_json::to_value(workspace).expect("original")
        );
    }
    #[test]
    fn read_only_decoder_keeps_complete_frame_and_business_refusals() {
        for fault in [
            "missing",
            "duplicate",
            "envelope",
            "numeric",
            "future",
            "unknown_business_root",
        ] {
            let mut workspace = serde_json::to_value(crate::domain::demo()).expect("model");
            workspace["_freight_format"] = serde_json::json!(FORMAT);
            if fault == "unknown_business_root" {
                workspace["unsupported_business_root"] = serde_json::json!([]);
            }
            let source = serde_json::to_string(&workspace).expect("source");
            let mut snapshot = stored(codec::pack(&source, CONFIG).expect("frame"));
            match fault {
                "missing" => {
                    snapshot.rows.remove(0);
                }
                "duplicate" => snapshot.rows.push(snapshot.rows[0].clone()),
                "envelope" => snapshot.rows[0].0 = "mismatched-root".into(),
                "numeric" => {
                    let mut block: Block =
                        serde_json::from_str(&snapshot.rows[0].2).expect("block");
                    block.items[0].payload =
                        RawValue::from_string("1e10000".into()).expect("JSON numeric literal");
                    snapshot.rows[0].2 = serde_json::to_string(&block).expect("corrupt block");
                }
                "future" => {
                    let mut header: serde_json::Value =
                        serde_json::from_str(&snapshot.header).expect("header");
                    header["_freight_format"] = serde_json::json!(999);
                    snapshot.header = serde_json::to_string(&header).expect("future header");
                }
                _ => {}
            }
            let other = Snapshot {
                header: snapshot.header.clone(),
                rows: snapshot.rows.clone(),
            };
            let read = snapshot
                .decode()
                .expect_err("read refuses corrupt original");
            let mutation = other
                .decode_mutation()
                .err()
                .expect("write baseline refuses corrupt original");
            assert_eq!(read, mutation, "{fault}");
        }
    }
    #[test]
    fn approval_projection_validates_its_complete_roots_without_decoding_unread_receipts() {
        let workspace = crate::domain::demo();
        let id = workspace.invoices[0].id.clone();
        let expected = workspace.invoice_approval_evidence(&id).expect("reference");
        let mut raw = serde_json::to_value(&workspace).expect("source");
        raw["_freight_format"] = serde_json::json!(FORMAT);
        raw["receipts"] =
            serde_json::json!({"unrelated":{"unsupported_original": "unread original retained"}});
        let source = serde_json::to_string(&raw).expect("original");
        assert!(stored(codec::pack(&source, CONFIG).expect("frame"))
            .decode()
            .is_err());
        let projected = approval_stored(codec::pack(&source, CONFIG).expect("frame"))
            .decode()
            .expect("scoped roots");
        assert_eq!(projected.evidence(&id).expect("evidence"), expected);
        for optional in ["inbox", "appointments", "stop_events"] {
            let mut frame = encode(&workspace).expect("frame");
            frame.roots.remove(optional);
            frame.root_names = frame.roots.keys().cloned().collect();
            assert_eq!(
                approval_stored(frame)
                    .decode()
                    .expect("absent optional")
                    .evidence(&id)
                    .expect("evidence"),
                expected
            );
        }
    }
    #[test]
    fn approval_projection_refuses_missing_duplicate_and_noncanonical_selected_originals() {
        for root in APPROVAL_ROOTS {
            for fault in [
                "missing",
                "ordinal",
                "address",
                "duplicate",
                "envelope",
                "numeric",
            ] {
                let mut snapshot = approval_stored(encode(&crate::domain::demo()).expect("frame"));
                let index = snapshot
                    .0
                    .rows
                    .iter()
                    .position(|(r, _, _)| r == root)
                    .expect("selected block");
                match fault {
                    "missing" => {
                        snapshot.0.rows.remove(index);
                    }
                    "duplicate" => snapshot.0.rows.push(snapshot.0.rows[index].clone()),
                    _ => {
                        let mut block: Block =
                            serde_json::from_str(&snapshot.0.rows[index].2).expect("block");
                        match fault {
                            "ordinal" => block.items[0].position = 0,
                            "address" => {
                                block.key = "a/noncanonical".into();
                                snapshot.0.rows[index].1 = block.key.clone();
                            }
                            "envelope" => block.root = "wrong-root".into(),
                            "numeric" => {
                                block.items[0].payload = RawValue::from_string(
                                    "{\"id\":\"owned\",\"future\":1e10000}".into(),
                                )
                                .expect("valid JSON literal")
                            }
                            _ => unreachable!("owned fault"),
                        }
                        snapshot.0.rows[index].2 =
                            serde_json::to_string(&block).expect("bad original");
                    }
                }
                assert!(snapshot.decode().is_err(), "{root}/{fault}");
            }
        }
    }
    #[test]
    fn approval_projection_requires_supported_manifest_kinds_and_literal_format() {
        for fault in [
            "unknown",
            "missing-required",
            "kind",
            "manifest",
            "format",
            "scalar",
        ] {
            let mut frame = encode(&crate::domain::demo()).expect("frame");
            match fault {
                "unknown" => {
                    frame.roots.insert(
                        "future_finance_policy".into(),
                        codec::Root {
                            kind: codec::Kind::Null,
                            size: 0,
                            scalar: RawValue::from_string("null".into()).expect("null"),
                        },
                    );
                }
                "missing-required" => {
                    frame.roots.remove("loads");
                }
                "kind" => frame.roots.get_mut("loads").expect("loads").kind = codec::Kind::Object,
                "manifest" => frame.root_names.clear(),
                "format" => {
                    frame
                        .roots
                        .get_mut("_freight_format")
                        .expect("marker")
                        .scalar = RawValue::from_string("21.0".into()).expect("marker")
                }
                "scalar" => {
                    frame.roots.get_mut("receipts").expect("receipts").scalar =
                        RawValue::from_string("true".into()).expect("scalar")
                }
                _ => unreachable!("owned fault"),
            }
            if fault != "manifest" {
                frame.root_names = frame.roots.keys().cloned().collect();
            }
            assert!(approval_stored(frame).decode().is_err(), "{fault}");
        }
    }
    #[test]
    fn canonical_snapshots_require_literal_integral_markers_before_business_decoding() {
        let mut roots: BTreeMap<String, Box<RawValue>> = serde_json::from_str(
            crate::write_json::encode(&crate::domain::demo(), FORMAT)
                .expect("source")
                .get(),
        )
        .expect("roots");
        for marker in [
            r#"{"$serde_json::private::RawValue":"21"}"#,
            "21.0",
            "20",
            "null",
            r#""21""#,
        ] {
            roots.insert(
                "_freight_format".into(),
                RawValue::from_string(marker.into()).expect("marker"),
            );
            let raw = serde_json::to_string(&roots).expect("raw source");
            let frame = codec::pack(&raw, CONFIG).expect("lossless arbitrary source");
            assert!(
                matches!(stored(frame).decode(),Err(error) if error==INCOMPATIBLE_STORAGE),
                "unsupported marker {marker}"
            );
        }
        for marker in ["21.0", "20", "null", r#""21""#] {
            let mut snapshot = stored(encode(&crate::domain::demo()).expect("frame"));
            snapshot.header = snapshot.header.replacen(
                "\"_freight_format\":21",
                &format!("\"_freight_format\":{marker}"),
                1,
            );
            assert!(matches!(snapshot.decode(),Err(error) if error==INCOMPATIBLE_STORAGE));
        }
    }
}
