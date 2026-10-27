//! Bounded order-scoped discovery; choosing a row still requires a fresh review.
use super::*;
pub const SHIPMENT_PAGE_SIZE: u32 = 25;

#[derive(Deserialize)]
struct ListedShipment {
    #[serde(flatten)]
    shipment: Shipment,
    shipment_number: String,
}
#[derive(Deserialize)]
struct ShipmentPage {
    shipments: Vec<ListedShipment>,
    total: u64,
    limit: u32,
    offset: u32,
    has_more: bool,
}
pub fn candidate_order(load: &Load) -> Result<OrderId, String> {
    let source = load
        .source_order
        .as_ref()
        .ok_or("Retained native order missing")?;
    if source["sourceSystem"] != "icommerce"
        || load.sync_order_id.as_deref() != source["id"].as_str()
    {
        return Err("Shipment discovery requires the load's retained native order identity".into());
    }
    source["id"]
        .as_str()
        .ok_or("Native order identity missing")?
        .parse()
        .map_err(|_| "Native order identity must be a UUID".into())
}
pub async fn shipment_candidates(
    w: &Workspace,
    load: &Load,
    c: &Connection,
    page: u32,
) -> Result<Value, String> {
    let order_id = candidate_order(load)?;
    let offset = page
        .checked_sub(1)
        .and_then(|n| n.checked_mul(SHIPMENT_PAGE_SIZE))
        .ok_or("Native shipment page is outside supported bounds")?;
    let raw = read(
        c,
        "/api/v1/shipments",
        &[
            ("order_id", order_id.to_string()),
            ("limit", SHIPMENT_PAGE_SIZE.to_string()),
            ("offset", offset.to_string()),
        ],
    )
    .await?;
    let native: ShipmentPage =
        serde_json::from_value(raw).map_err(|_| "Invalid native shipment page")?;
    if native.limit != SHIPMENT_PAGE_SIZE
        || native.offset != offset
        || native.shipments.len() > SHIPMENT_PAGE_SIZE as usize
        || native.has_more && native.shipments.len() != SHIPMENT_PAGE_SIZE as usize
    {
        return Err("Native shipment page does not match requested bounds".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    // Validate the entire page before exposing any candidate or partial result.
    for row in &native.shipments {
        let s = &row.shipment;
        if s.order_id != order_id
            || !ids.insert(s.id)
            || s.version < 0
            || [(&row.shipment_number, 200), (&s.status, 100)]
                .iter()
                .any(|(text, max)| {
                    text.trim().is_empty()
                        || text.len() > *max
                        || text.chars().any(char::is_control)
                })
        {
            return Err("Native shipment page contains an invalid or unrelated identity".into());
        }
    }
    let active: Vec<Value> = w
        .outbox
        .iter()
        .filter(|j| {
            j.load_id == load.id
                && j.destination == "stateset-icommerce"
                && matches!(
                    j.status.as_str(),
                    "pending_handoff" | "sending" | "retry" | "held"
                )
        })
        .map(|j| json!({"id":j.id,"status":j.status}))
        .collect();
    let items: Vec<Value> = native.shipments.into_iter().map(|row| {
        let review = guard_link(w, &load.id, row.shipment.id)
            .and_then(|_| tracking_plan(load, &row.shipment));
        let (state, reason, plan) = match review {
            Err(reason) => ("blocked", reason, None),
            Ok(None) => ("up_to_date", "Native shipment already reflects this freight state".into(), None),
            Ok(Some(plan)) if !active.is_empty() => ("blocked", "Resolve the existing native handoff in Integrations before another step".into(), Some(plan)),
            Ok(Some(plan)) => ("ready_for_review", format!("Review {} → {}", plan.from_status, plan.next_status), Some(plan)),
        };
        json!({"shipment":row.shipment,"shipment_number":row.shipment_number,"state":state,"reason":reason,"plan":plan})
    }).collect();
    Ok(
        json!({"load_id":load.id,"expected_load_version":load.version,"order_id":order_id,"page":page,"page_size":SHIPMENT_PAGE_SIZE,"total":native.total,"has_more":native.has_more,"active_handoffs":active,"items":items}),
    )
}
