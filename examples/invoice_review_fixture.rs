//! Seed one explicitly owned browser-qualification tenant; never overwrite it.
#[path = "../tests/support/canonical.rs"]
mod canonical;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tenant = std::env::args().nth(1).ok_or("Owned tenant required")?;
    if tenant != "browser-invoice-review" {
        return Err("Only the owned browser invoice-review tenant is supported".into());
    }
    let url = std::env::var("FREIGHT_E2E_DATABASE_URL")?;
    let parsed = reqwest::Url::parse(&url)?;
    if !matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1")) {
        return Err("Use the owned loopback browser qualification database".into());
    }
    let pool = sqlx::PgPool::connect(&url).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM freight_workspaces WHERE tenant_id=$1)")
            .bind(&tenant)
            .fetch_one(&pool)
            .await?;
    if exists {
        return Err("Retain the previous fixture and use a fresh browser database".into());
    }
    let mut workspace = stateset_freight::domain::demo();
    // Retain a historical decision with no independent-review metadata beside
    // the current review flow. The UI must not invent named review provenance.
    let historical = workspace.invoices[1].id.clone();
    let load = workspace
        .loads
        .iter()
        .find(|v| v.id == workspace.invoices[1].load_id)
        .ok_or("Owned historical load missing")?;
    let agreed = workspace.charge_basis(load, "buy")?.total_amount_cents;
    workspace.apply("invoice.resolve", &historical, serde_json::json!({"expected_amount_cents":workspace.invoices[1].amount_cents,"amount_cents":agreed,"note":"Owned legacy invoice corrected before the independent-review policy"}))?;
    workspace.apply("invoice.approve", &historical, serde_json::json!({}))?;
    canonical::put_workspace(&pool, &tenant, &workspace).await?;
    pool.close().await;
    println!("Seeded isolated synthetic invoice review fixture");
    Ok(())
}
