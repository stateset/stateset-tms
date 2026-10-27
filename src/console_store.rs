//! Warm SQL pages use catalog admission only. Cold projection rebuilds obtain
//! normal snapshot/workspace admission before occupying a connection or row lock.
use super::*;
use crate::console_catalog::{
    self as console, Balance, Cursor, Direction, Projection, Query, View,
};
use chrono::Utc;
use sqlx::Row;
use std::{sync::LazyLock, time::Instant};

pub(crate) const PAGE_SQL: &str = include_str!("console_index_page.sql");
pub(crate) const SEARCH_SQL: &str = include_str!("console_search_page.sql");
static QUERIES: LazyLock<[String; 16]> = LazyLock::new(|| queries(PAGE_SQL));
static SEARCH_QUERIES: LazyLock<[String; 16]> = LazyLock::new(|| queries(SEARCH_SQL));
fn queries(template: &str) -> [String; 16] {
    std::array::from_fn(|mask| {
        let mut filters=String::from(" AND ($5::text='' OR EXISTS(SELECT 1 FROM unnest(s.search_values) AS v WHERE strpos(v,$5)>0))");
        if mask & 1 != 0 {
            filters.push_str(" AND s.balance=$3");
        }
        if mask & 2 != 0 {
            filters.push_str(" AND s.direction=$4");
        }
        if mask & 4 != 0 {
            filters.push_str(" AND s.attention=true");
        }
        if mask & 8 != 0 {
            filters.push_str(" AND s.overdue=true");
        }
        template.replace("/*FILTERS*/", &filters)
    })
}
fn mask(query: &Query) -> usize {
    usize::from(!matches!(
        query.balance.unwrap_or_default(),
        Balance::All | Balance::Overdue
    )) | (usize::from(query.direction.unwrap_or_default() != Direction::All) << 1)
        | (usize::from(query.attention.unwrap_or(false)) << 2)
        | (usize::from(query.balance == Some(Balance::Overdue)) << 3)
}
fn code<T: serde::Serialize>(value: T) -> Result<String, String> {
    serde_json::to_value(value)
        .map_err(internal)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "Internal storage error".into())
}
fn cursor(query: &Query) -> Result<Option<Cursor>, String> {
    query
        .cursor
        .as_ref()
        .map(|raw| {
            let c: Cursor = hex::decode(raw)
                .ok()
                .and_then(|v| serde_json::from_slice(&v).ok())
                .ok_or("Invalid console page cursor")?;
            if c.version != 2 || c.after.is_empty() || c.after.len() > 202 || c.scope.len() != 64 {
                return Err("Invalid console page cursor; restart from the first page".into());
            }
            Ok(c)
        })
        .transpose()
}
impl Store {
    pub async fn console_page(
        &self,
        tenant: &str,
        principal: &crate::auth::Principal,
        query: Query,
    ) -> Result<Value, String> {
        query.validate()?;
        cursor(&query)?;
        if self.pool.is_none() {
            let tenant = tenant.to_owned();
            let owner = tenant.clone();
            let principal = principal.clone();
            return self
                .read_process(&tenant, move |w, _| {
                    w.console_page(&owner, &principal, query)
                })
                .await;
        }
        let permit = self.catalog_admission.acquire(tenant).await?;
        let store = self.clone();
        let tenant = tenant.to_owned();
        let principal = principal.clone();
        tokio::spawn(async move {
            store
                .indexed_console_page(tenant, principal, query, permit)
                .await
        })
        .await
        .map_err(internal)?
    }
    async fn indexed_console_page(
        &self,
        tenant: String,
        principal: crate::auth::Principal,
        query: Query,
        permit: Arc<OwnedSemaphorePermit>,
    ) -> Result<Value, String> {
        let (limit, q) = query.validate()?;
        let after = cursor(&query)?;
        let _permit = permit.clone();
        let pool = self.pool.as_ref().ok_or("Internal storage error")?;
        for _ in 0..3 {
            let date = Utc::now().date_naive();
            let mut tx = self
                .index_transaction(pool, &tenant, Some(permit.clone()))
                .await?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            require_format(&mut tx).await?;
            let statements = if q.is_empty() {
                &QUERIES
            } else {
                &SEARCH_QUERIES
            };
            let row = sqlx::query(&statements[mask(&query)])
                .bind(&tenant)
                .bind(query.view.code())
                .bind(code(query.balance.unwrap_or_default())?)
                .bind(code(query.direction.unwrap_or_default())?)
                .bind(&q)
                .bind(after.as_ref().map(|c| c.after.as_str()).unwrap_or(""))
                .bind((limit + 1) as i64)
                .bind(date.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(database_error)?;
            tx.commit().await.map_err(database_error)?;
            if row.as_ref().is_some_and(|r| r.get::<bool, _>("ready")) {
                let row = row.ok_or("Internal storage error")?;
                let errors: Value =
                    serde_json::from_str(row.get::<&str, _>("errors")).map_err(internal)?;
                if let Some(error) = errors[query.view.code()].as_str() {
                    return Err(error.to_owned());
                }
                let digest: String = row.get("identities");
                let scope = console::scope(&tenant, &principal, &query, limit, &q, &digest)?;
                if after
                    .as_ref()
                    .is_some_and(|c| c.scope != scope || !row.get::<bool, _>("anchor"))
                {
                    return Err(
                        "Console page cursor does not match these records or filters".into(),
                    );
                }
                let mut rows: Vec<Value> =
                    serde_json::from_str(row.get::<&str, _>("rows")).map_err(internal)?;
                let next = if rows.len() > limit {
                    rows.truncate(limit);
                    let after = rows
                        .last()
                        .and_then(|r| r["key"].as_str())
                        .ok_or("Internal storage error")?;
                    Some(hex::encode(
                        serde_json::to_vec(&Cursor {
                            version: 2,
                            scope,
                            after: after.to_owned(),
                        })
                        .map_err(internal)?,
                    ))
                } else {
                    None
                };
                let finance: Value = if query.view == View::Integrations {
                    Value::Null
                } else {
                    serde_json::from_str(row.get::<&str, _>("finance")).map_err(internal)?
                };
                let mut page = json!({"tenant_id":tenant,"view":query.view,"mode":"console","limit":limit,"total":row.get::<i64,_>("total"),"next_cursor":next,
                        "totals_scope":"workspace","finance":finance,"bills":[],"settlements":[],"outbox":[],"inbox":[]});
                for row in rows {
                    let key = row["collection"].as_str().ok_or("Internal storage error")?;
                    page.get_mut(key)
                        .and_then(Value::as_array_mut)
                        .ok_or("Internal storage error")?
                        .push(row["record"].clone());
                }
                if date != Utc::now().date_naive() {
                    continue;
                }
                return Ok(page);
            }
            if !self.rebuild_console(&tenant, permit.clone()).await? {
                return Workspace::default().console_page(&tenant, &principal, query);
            }
        }
        Err(DATABASE_BUSY.into())
    }
    async fn rebuild_console(
        &self,
        tenant: &str,
        page: Arc<OwnedSemaphorePermit>,
    ) -> Result<bool, String> {
        let read = self.read_permit(tenant).await?;
        let deadline = self.write_turns.deadline();
        let turn = self.write_turns.request(tenant)?.wait(deadline).await?;
        let work = self.workspace_work(tenant).await?;
        let pool = self.pool.as_ref().ok_or("Internal storage error")?;
        let transaction = tokio::time::timeout_at(deadline, pool.begin())
            .await
            .map_err(|_| database_error(sqlx::Error::PoolTimedOut))?
            .map_err(database_error)?;
        let mut tx = WriteTransaction::new(transaction, turn);
        tx.hold_work(work.clone());
        tx.hold_additional_work(page);
        tx.hold_additional_work(Arc::new(read));
        self.configure_workspace_transaction(&mut tx).await?;
        require_format(&mut tx).await?;
        let started = Instant::now();
        let header: Option<String> = sqlx::query_scalar(
            "SELECT data::text FROM freight_workspaces WHERE tenant_id=$1 FOR UPDATE",
        )
        .bind(tenant)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        tracing::debug!(
            phase = "console_header_lock",
            elapsed_us = started.elapsed().as_micros() as u64
        );
        let Some(header) = header else {
            tx.commit().await.map_err(database_error)?;
            return Ok(false);
        };
        // Another process may have published while we waited for the original.
        let ready:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM freight_console_state WHERE tenant_id=$1 AND ready AND as_of=$2::text::date)")
            .bind(tenant).bind(Utc::now().date_naive().to_string()).fetch_one(&mut *tx).await.map_err(database_error)?;
        if ready {
            tx.commit().await.map_err(database_error)?;
            return Ok(true);
        }
        // Only the lock winner that still needs a rebuild transfers originals.
        // The tenant row remains locked through snapshot, publication and commit.
        let snapshot = canonical::snapshot_from_header(&mut tx, tenant, header).await?;
        tracing::debug!(
            phase = "console_snapshot",
            elapsed_us = started.elapsed().as_micros() as u64
        );
        let (mut tx, projection) = tokio::task::spawn_blocking(move || {
            let _work = work;
            let started = Instant::now();
            let workspace = snapshot.decode()?;
            tracing::debug!(
                phase = "console_decode",
                elapsed_us = started.elapsed().as_micros() as u64
            );
            let started = Instant::now();
            let projection = Projection::build(&workspace);
            tracing::debug!(
                phase = "console_projection",
                elapsed_us = started.elapsed().as_micros() as u64,
                rows = projection.rows.len()
            );
            Ok::<_, String>((tx, projection))
        })
        .await
        .map_err(internal)??;
        let started = Instant::now();
        publish(&mut tx, tenant, projection).await?;
        tracing::debug!(
            phase = "console_publish",
            elapsed_us = started.elapsed().as_micros() as u64
        );
        tx.commit().await.map_err(database_error)?;
        Ok(true)
    }
}
async fn publish(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    projection: Projection,
) -> Result<(), String> {
    sqlx::query("INSERT INTO freight_console_state(tenant_id,ready,as_of,finance,errors) VALUES($1,false,$2::text::date,$3,$4) ON CONFLICT(tenant_id) DO UPDATE SET ready=false,as_of=EXCLUDED.as_of,finance=EXCLUDED.finance,errors=EXCLUDED.errors")
        .bind(tenant).bind(projection.date.to_string()).bind(&projection.finance).bind(&projection.errors).execute(&mut *connection).await.map_err(database_error)?;
    sqlx::query("DELETE FROM freight_console_records WHERE tenant_id=$1")
        .bind(tenant)
        .execute(&mut *connection)
        .await
        .map_err(database_error)?;
    for batch in projection.rows.chunks(64) {
        // Binary arrays avoid cloning the complete batch into a JSON value and
        // reparsing repeated field names in PostgreSQL. JSON is retained only
        // for the original summary values and variable-length search arrays.
        let views: Vec<_> = batch.iter().map(|r| r.view).collect();
        let keys: Vec<_> = batch.iter().map(|r| r.record_key.as_str()).collect();
        let collections: Vec<_> = batch.iter().map(|r| r.collection).collect();
        let summaries: Vec<_> = batch
            .iter()
            .map(|r| sqlx::types::Json(&r.summary))
            .collect();
        let searches: Vec<_> = batch
            .iter()
            .map(|r| sqlx::types::Json(&r.search_values))
            .collect();
        let balances: Vec<_> = batch.iter().map(|r| r.balance).collect();
        let directions: Vec<_> = batch.iter().map(|r| r.direction).collect();
        let attention: Vec<_> = batch.iter().map(|r| r.attention).collect();
        let overdue: Vec<_> = batch.iter().map(|r| r.overdue).collect();
        sqlx::query("INSERT INTO freight_console_records SELECT $1,p.view,p.record_key,p.collection,p.summary,ARRAY(SELECT jsonb_array_elements_text(p.search_values)),p.balance,p.direction,p.attention,p.overdue FROM unnest($2::text[],$3::text[],$4::text[],$5::jsonb[],$6::jsonb[],$7::text[],$8::text[],$9::boolean[],$10::boolean[]) AS p(view,record_key,collection,summary,search_values,balance,direction,attention,overdue)")
            .bind(tenant).bind(views).bind(keys).bind(collections).bind(summaries)
            .bind(searches).bind(balances).bind(directions).bind(attention).bind(overdue)
            .execute(&mut *connection).await.map_err(database_error)?;
    }
    sqlx::query("UPDATE freight_console_state SET ready=true WHERE tenant_id=$1")
        .bind(tenant)
        .execute(connection)
        .await
        .map_err(database_error)?;
    Ok(())
}
#[cfg(test)]
#[path = "console_index_tests.rs"]
mod tests;
