//! Typed boundaries between iCommerce order fulfillment and freight operations.
//! Freight remains authoritative for load planning, rates and execution.
//! Amounts are exact integer minor units in one explicit currency; no
//! workflow converts currency implicitly. Reporting conversion is available
//! only through [`convert_money`] with caller-supplied, timestamped rates.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub use stateset_pricing::{
    ConversionResult, CurrencyConverter, ExchangeRate, RoundingMode, RoundingPolicy,
};
pub use stateset_primitives::{
    CurrencyCode, CustomerId, Money, MoneyWire, OrderId, ProductId, ShipmentId,
};

pub type Result<T> = std::result::Result<T, String>;

/// Parse an ISO-style three-letter currency code, normalized to uppercase.
pub fn parse_currency(code: &str) -> Result<CurrencyCode> {
    code.parse::<CurrencyCode>()
        .map_err(|_| "Currency must be a three-letter code".into())
}

/// Decimal places in one major unit for `currency` (2 for USD/EUR, 0 for JPY,
/// 3 for BHD), from the shared iCommerce currency table.
pub fn decimal_places(currency: CurrencyCode) -> u32 {
    stateset_pricing::minor_units_for_currency(currency.as_str())
}

/// Convert integer minor units without a float, rounding, or range loss.
pub fn from_minor(minor: i64, currency: CurrencyCode) -> Money {
    let places = decimal_places(currency);
    let magnitude = minor.unsigned_abs();
    let sign = if minor < 0 { "-" } else { "" };
    let amount = if places == 0 {
        format!("{sign}{magnitude}")
    } else {
        let factor = 10_u64.pow(places);
        format!(
            "{sign}{}.{:0width$}",
            magnitude / factor,
            magnitude % factor,
            width = places as usize
        )
    };
    Money::from_decimal_str(&amount, currency)
        .expect("i64 minor units fit exact Decimal in the currency scale")
}
/// Validate even Money constructed through upstream's unchecked `new` constructor.
pub fn to_minor(money: Money) -> Result<i64> {
    let places = decimal_places(money.currency());
    let amount = money.amount().normalize();
    let scale = amount.scale();
    if scale > places {
        return Err(format!(
            "{} amounts must be exact minor units; rounding is not permitted",
            money.currency()
        ));
    }
    let factor = 10_i128.pow(places - scale);
    amount
        .mantissa()
        .checked_mul(factor)
        .and_then(|n| n.try_into().ok())
        .ok_or_else(|| "Amount exceeds signed 64-bit minor units".into())
}
pub fn parse_money(amount: &str, currency: CurrencyCode) -> Result<i64> {
    let money = Money::from_decimal_str(amount, currency)
        .map_err(|_| "Invalid exact money amount or currency precision")?;
    to_minor(money)
}
pub fn format_money(minor: i64, currency: CurrencyCode) -> String {
    format!(
        "{:.*}",
        decimal_places(currency) as usize,
        from_minor(minor, currency).amount()
    )
}
/// Convert for reporting only, using explicit caller-supplied rates. The
/// result is rounded half-up to the target currency's minor units; the
/// original amount and the conversion record remain authoritative evidence,
/// so callers must retain both rather than overwrite settled amounts.
pub fn convert_money(
    money: Money,
    to: CurrencyCode,
    rates: &[ExchangeRate],
) -> Result<(Money, ConversionResult)> {
    let mut converter = CurrencyConverter::with_base_currency(money.currency().as_str());
    for rate in rates {
        converter.add_rate(rate.clone());
    }
    let result = converter
        .convert(money.amount(), money.currency().as_str(), to.as_str())
        .map_err(|_| "No exchange rate path for currency conversion")?;
    let rounded = stateset_pricing::round(
        result.amount,
        &RoundingPolicy::new(RoundingMode::HalfUp, decimal_places(to)),
    );
    let converted = Money::try_new(rounded, to)
        .map_err(|_| "Converted amount exceeds target currency scale")?;
    Ok((converted, result))
}

/// Convert integer USD minor units without a float, rounding, or range loss.
pub fn usd_from_minor(cents: i64) -> Money {
    from_minor(cents, CurrencyCode::USD)
}
/// Validate even Money constructed through upstream's unchecked `new` constructor.
pub fn usd_to_minor(money: Money) -> Result<i64> {
    if money.currency() != CurrencyCode::USD {
        return Err("Expected a USD amount; no implicit currency conversion is permitted".into());
    }
    to_minor(money)
}
pub fn parse_usd(amount: &str, currency: CurrencyCode) -> Result<i64> {
    let money = Money::from_decimal_str(amount, currency)
        .map_err(|_| "Invalid exact money amount or currency precision")?;
    usd_to_minor(money)
}
pub fn format_usd(cents: i64) -> String {
    format_money(cents, CurrencyCode::USD)
}

/// The order fields actually exposed by stateset-http's /api/v1/orders DTO.
/// Read DTOs permit future extra fields; only these validated fields enter a snapshot.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Order {
    pub id: OrderId,
    pub order_number: String,
    pub customer_id: CustomerId,
    pub status: String,
    pub total_amount: String,
    pub currency: CurrencyCode,
    pub payment_status: String,
    pub fulfillment_status: String,
    pub items: Vec<OrderItem>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OrderItem {
    pub id: uuid::Uuid,
    pub product_id: ProductId,
    pub sku: String,
    pub name: String,
    pub quantity: i32,
    #[serde(default)]
    pub shipped_quantity: i32,
    pub unit_price: String,
    pub total: String,
}
fn label(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
impl Order {
    pub fn validate(&self) -> Result<()> {
        if !label(&self.order_number, 200)
            || !label(&self.status, 100)
            || !label(&self.payment_status, 100)
            || !label(&self.fulfillment_status, 100)
            || self.updated_at < self.created_at
            || self.items.is_empty()
            || self.items.len() > 1000
        {
            return Err("Invalid iCommerce order identity, revision or line inventory".into());
        }
        if parse_money(&self.total_amount, self.currency)? < 0 {
            return Err("Order total cannot be negative".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for item in &self.items {
            if !ids.insert(item.id)
                || item.quantity <= 0
                || item.shipped_quantity < 0
                || item.shipped_quantity > item.quantity
                || !label(&item.sku, 200)
                || !label(&item.name, 200)
                || parse_money(&item.unit_price, self.currency)? < 0
                || parse_money(&item.total, self.currency)? < 0
            {
                return Err("Invalid iCommerce order line or fulfillment quantity".into());
            }
        }
        Ok(())
    }
    pub fn importable(&self) -> bool {
        matches!(self.status.as_str(), "pending" | "confirmed" | "processing")
            && self.items.iter().all(|item| item.shipped_quantity == 0)
            && matches!(self.fulfillment_status.as_str(), "unfulfilled" | "pending")
    }
    pub fn snapshot(&self) -> Result<Value> {
        self.validate()?;
        let status = match self.status.as_str() {
            "confirmed" => "processing",
            "refunded" => "cancelled",
            other => other,
        };
        Ok(json!({
            "id":self.id,"orderNumber":self.order_number,"customerId":self.customer_id,
            "status":status,"updatedAt":self.updated_at.to_rfc3339(),"createdAt":self.created_at.to_rfc3339(),
            "currency":self.currency,"totalAmount":self.total_amount,"paymentStatus":self.payment_status,
            "fulfillmentStatus":self.fulfillment_status,"sourceSystem":"icommerce","commerceStatus":self.status,
            "importable":self.importable(),
            "items":self.items.iter().map(|item|json!({"id":item.id,"productId":item.product_id,"sku":item.sku,"name":item.name,"quantity":item.quantity,"shippedQuantity":item.shipped_quantity,"unitPrice":item.unit_price,"total":item.total})).collect::<Vec<_>>()
        }))
    }
}
pub fn order_snapshot(value: Value, expected: OrderId) -> Result<Value> {
    let order: Order = serde_json::from_value(value).map_err(|_| {
        "Invalid iCommerce order response; exact decimal strings and typed IDs are required"
    })?;
    if order.id != expected {
        return Err("iCommerce order identity mismatch".into());
    }
    order.snapshot()
}
/// Native order totals describe merchandise; they never become a freight rate.
/// The operator must supply the freight customer, physical route and buy/sell plan.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrderPage {
    pub orders: Vec<Order>,
    pub total: usize,
    pub limit: u32,
    pub offset: u32,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub next_cursor: Option<String>,
}
impl OrderPage {
    pub fn normalized(self, page: u32) -> Result<Value> {
        if self.orders.len() > 100
            || self.limit != 100
            || self.offset != page.saturating_sub(1).saturating_mul(100)
        {
            return Err("iCommerce order page does not match requested bounds".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        let items = self
            .orders
            .iter()
            .map(|order| {
                if !ids.insert(order.id) {
                    return Err("Duplicate order in iCommerce page".into());
                }
                order.snapshot()
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"items":items,"page":page,"pageSize":100,"total":self.total,"sourceSystem":"icommerce"}),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn order() -> Order {
        serde_json::from_value(json!({"id":"00000000-0000-4000-8000-000000000001","order_number":"ORDER-1","customer_id":"00000000-0000-4000-8000-000000000002","status":"confirmed","total_amount":"19.99","currency":"USD","payment_status":"paid","fulfillment_status":"unfulfilled","items":[{"id":"00000000-0000-4000-8000-000000000003","product_id":"00000000-0000-4000-8000-000000000004","sku":"SKU-1","name":"A pallet of goods","quantity":2,"shipped_quantity":0,"unit_price":"9.99","total":"19.98"}],"created_at":"2026-10-19T00:00:00Z","updated_at":"2026-10-19T01:00:00Z"})).expect("fixture")
    }
    #[test]
    fn usd_round_trip_is_exact_at_signed_limits_and_never_uses_float() {
        for cents in [i64::MIN, i64::MAX, -101, -1, 0, 1, 99, 100, 123456789] {
            assert_eq!(
                usd_to_minor(usd_from_minor(cents)).expect("round trip"),
                cents
            );
            assert_eq!(
                parse_usd(&format_usd(cents), CurrencyCode::USD).expect("parse"),
                cents
            );
        }
        assert!(parse_usd("1.001", CurrencyCode::USD).is_err());
        assert!(parse_usd("92233720368547758.08", CurrencyCode::USD).is_err());
        assert!(parse_usd("1.00", CurrencyCode::EUR).is_err());
        assert!(parse_usd("NaN", CurrencyCode::USD).is_err());
    }
    #[test]
    fn typed_snapshot_retains_native_state_without_inventing_a_freight_rate() {
        let order = order();
        let snapshot = order.snapshot().expect("snapshot");
        assert_eq!(snapshot["status"], "processing");
        assert_eq!(snapshot["commerceStatus"], "confirmed");
        assert_eq!(snapshot["totalAmount"], "19.99");
        assert_eq!(snapshot["sourceSystem"], "icommerce");
        assert!(snapshot.get("sell_cents").is_none());
        assert!(snapshot.get("buy_cents").is_none());
        assert!(snapshot.get("shippingAddress").is_none());
        assert!(snapshot["importable"].as_bool().expect("flag"));
        let other: OrderId = "00000000-0000-4000-8000-000000000099".parse().expect("ID");
        assert!(order_snapshot(serde_json::to_value(order).expect("value"), other).is_err());
    }
    #[test]
    fn completed_cancelled_partial_and_unknown_orders_are_not_forward_imports() {
        for status in [
            "shipped",
            "delivered",
            "cancelled",
            "refunded",
            "on_hold",
            "unknown",
        ] {
            let mut o = order();
            o.status = status.into();
            assert!(!o.importable());
            o.snapshot().expect("retained for source review");
        }
        let mut o = order();
        o.items[0].shipped_quantity = 1;
        assert!(!o.importable());
        o.items[0].shipped_quantity = 3;
        assert!(o.validate().is_err());
    }
    #[test]
    fn malformed_amounts_revisions_and_duplicate_lines_fail_closed() {
        let mut o = order();
        o.currency = CurrencyCode::EUR;
        o.validate()
            .expect("EUR amounts are exact in their own currency");
        o.items[0].unit_price = "0.001".into();
        assert!(o.validate().is_err());
        let mut o = order();
        o.items[0].unit_price = "0.001".into();
        assert!(o.validate().is_err());
        let mut o = order();
        o.items.push(o.items[0].clone());
        assert!(o.validate().is_err());
        let mut o = order();
        o.updated_at = o.created_at - chrono::Duration::seconds(1);
        assert!(o.validate().is_err());
        let mut raw = serde_json::to_value(order()).expect("value");
        raw["total_amount"] = json!(19.99);
        assert!(order_snapshot(raw, order().id).is_err());
    }
}

/// Native shipment identity/version and manifest, used only for explicit status handoff.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Shipment {
    pub id: ShipmentId,
    pub order_id: OrderId,
    pub status: String,
    pub version: i32,
    pub items: Vec<ShipmentItem>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShipmentItem {
    pub sku: String,
    pub product_id: Option<ProductId>,
    pub quantity: i32,
}
/// One lifecycle step. Packing, stock allocation and carrier-label purchase stay in iCommerce.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingPlan {
    pub shipment_id: ShipmentId,
    pub order_id: OrderId,
    pub expected_version: i32,
    pub from_status: String,
    pub next_status: String,
    pub target_status: String,
}
impl TrackingPlan {
    pub fn new(shipment: &Shipment, source: &Value, freight_status: &str) -> Result<Option<Self>> {
        let order: OrderId = source["id"]
            .as_str()
            .ok_or("Retained source order ID missing")?
            .parse()
            .map_err(|_| "Invalid retained order ID")?;
        if source["sourceSystem"] != "icommerce"
            || shipment.order_id != order
            || shipment.version < 0
            || shipment.version.checked_add(1).is_none()
        {
            return Err(
                "Shipment order, provider or version does not match the imported freight source"
                    .into(),
            );
        }
        let manifest=|items:Vec<(String,String,i64)>|->Result<std::collections::BTreeMap<(String,String),i64>> {
            let mut result=std::collections::BTreeMap::<(String,String),i64>::new();
            for (product,sku,quantity) in items {
                if quantity<=0 || !label(&sku,200){return Err("Shipment manifests require positive quantities and SKU identity".into());}
                let total=result.entry((product,sku)).or_default();*total=total.checked_add(quantity).ok_or("Manifest quantity exhausted")?;
            }
            Ok(result)
        };
        let lines = source["items"]
            .as_array()
            .filter(|v| !v.is_empty() && v.len() <= 1000)
            .ok_or("Retained order manifest missing")?;
        let expected = manifest(
            lines
                .iter()
                .map(|line| {
                    Ok((
                        line["productId"]
                            .as_str()
                            .ok_or("Order product ID missing")?
                            .into(),
                        line["sku"].as_str().ok_or("Order SKU missing")?.into(),
                        line["quantity"].as_i64().ok_or("Order quantity missing")?,
                    ))
                })
                .collect::<Result<_>>()?,
        )?;
        if shipment.items.is_empty() || shipment.items.len() > 1000 {
            return Err("Native shipment manifest missing or oversized".into());
        }
        let actual = manifest(
            shipment
                .items
                .iter()
                .map(|line| {
                    Ok((
                        line.product_id
                            .ok_or("Shipment product ID missing")?
                            .to_string(),
                        line.sku.clone(),
                        i64::from(line.quantity),
                    ))
                })
                .collect::<Result<_>>()?,
        )?;
        if actual != expected {
            return Err("Native shipment manifest must cover exactly the imported order quantities; partial or unrelated shipments cannot be advanced by this load".into());
        }
        let target =
            match freight_status {
                "in_transit" => "in_transit",
                "delivered" => "delivered",
                _ => return Err(
                    "Only physically in-transit or delivered freight can update a native shipment"
                        .into(),
                ),
            };
        if shipment.status == target
            || target == "in_transit"
                && matches!(shipment.status.as_str(), "out_for_delivery" | "delivered")
        {
            return Ok(None);
        }
        let next=match shipment.status.as_str() {
            "shipped"|"failed"=>"in_transit",
            "in_transit" if target=="delivered"=>"out_for_delivery",
            "out_for_delivery" if target=="delivered"=>"delivered",
            _=>return Err("Complete packing and carrier handoff in iCommerce first; freight cannot skip or reverse its lifecycle".into()),
        };
        Ok(Some(Self {
            shipment_id: shipment.id,
            order_id: order,
            expected_version: shipment.version,
            from_status: shipment.status.clone(),
            next_status: next.into(),
            target_status: target.into(),
        }))
    }
    pub fn body(&self) -> Value {
        json!({"expected_version":self.expected_version,"status":self.next_status})
    }
    pub fn confirms(&self, shipment: &Shipment) -> bool {
        shipment.id == self.shipment_id
            && shipment.order_id == self.order_id
            && Some(shipment.version) == self.expected_version.checked_add(1)
            && shipment.status == self.next_status
    }
}

#[cfg(test)]
mod tracking_tests {
    use super::*;
    fn source() -> Value {
        json!({"id":"00000000-0000-4000-8000-000000000001","sourceSystem":"icommerce","items":[{"productId":"00000000-0000-4000-8000-000000000004","sku":"SKU-1","quantity":2}]})
    }
    fn shipment() -> Shipment {
        serde_json::from_value(json!({"id":"00000000-0000-4000-8000-000000000005","order_id":"00000000-0000-4000-8000-000000000001","status":"shipped","version":3,"items":[{"product_id":"00000000-0000-4000-8000-000000000004","sku":"SKU-1","quantity":2}]})).expect("fixture")
    }
    #[test]
    fn native_lifecycle_uses_legal_single_steps_and_never_regresses() {
        let mut s = shipment();
        for (from, next) in [
            ("shipped", "in_transit"),
            ("failed", "in_transit"),
            ("in_transit", "out_for_delivery"),
            ("out_for_delivery", "delivered"),
        ] {
            s.status = from.into();
            let plan = TrackingPlan::new(&s, &source(), "delivered")
                .expect("plan")
                .expect("step");
            assert_eq!(plan.next_status, next);
            assert_eq!(plan.body(), json!({"expected_version":3,"status":next}));
            s.status = next.into();
            s.version = 4;
            assert!(plan.confirms(&s));
            s.version = 5;
            assert!(!plan.confirms(&s));
            s.version = 3;
        }
        for status in ["in_transit", "out_for_delivery", "delivered"] {
            s.status = status.into();
            assert!(TrackingPlan::new(&s, &source(), "in_transit")
                .expect("no regression")
                .is_none());
        }
        s.status = "delivered".into();
        assert!(TrackingPlan::new(&s, &source(), "delivered")
            .expect("converged")
            .is_none());
        for status in [
            "pending",
            "processing",
            "ready_to_ship",
            "cancelled",
            "returned",
            "on_hold",
            "unknown",
        ] {
            s.status = status.into();
            assert!(TrackingPlan::new(&s, &source(), "delivered").is_err());
        }
    }
    #[test]
    fn minor_units_round_trip_across_currency_scales() {
        for (code, minor, text) in [
            ("USD", 12_345, "123.45"),
            ("EUR", 12_345, "123.45"),
            ("JPY", 12_345, "12345"),
            ("BHD", 12_345, "12.345"),
            ("USD", -5, "-0.05"),
            ("JPY", -5, "-5"),
        ] {
            let currency = parse_currency(code).expect("code");
            assert_eq!(format_money(minor, currency), text);
            assert_eq!(to_minor(from_minor(minor, currency)).expect("minor"), minor);
            assert_eq!(parse_money(text, currency).expect("parse"), minor);
        }
        assert!(parse_money("1.5", parse_currency("JPY").expect("code")).is_err());
        assert!(parse_money("1.001", parse_currency("EUR").expect("code")).is_err());
        assert_eq!(
            parse_money("1.001", parse_currency("BHD").expect("code")).expect("parse"),
            1001
        );
        assert_eq!(parse_currency("eur").expect("lowercase").as_str(), "EUR");
        assert!(parse_currency("EU").is_err());
        assert!(parse_currency("E1R").is_err());
        assert_eq!(decimal_places(CurrencyCode::USD), 2);
        assert_eq!(decimal_places(CurrencyCode::JPY), 0);
    }
    #[test]
    fn reporting_conversion_uses_explicit_rates_and_target_scale() {
        let rate = ExchangeRate {
            from: "USD".into(),
            to: "EUR".into(),
            rate: "0.92".parse().expect("rate"),
            as_of: Utc::now(),
        };
        let (converted, record) = convert_money(
            usd_from_minor(10_000),
            CurrencyCode::EUR,
            std::slice::from_ref(&rate),
        )
        .expect("convert");
        assert_eq!(to_minor(converted).expect("minor"), 92_00);
        assert_eq!(record.from, "USD");
        assert_eq!(record.to, "EUR");
        let yen_rates = [
            rate,
            ExchangeRate {
                from: "USD".into(),
                to: "JPY".into(),
                rate: "150".parse().expect("rate"),
                as_of: Utc::now(),
            },
        ];
        let (yen, _) = convert_money(usd_from_minor(10_000), CurrencyCode::JPY, &yen_rates)
            .expect("convert yen");
        assert_eq!(to_minor(yen).expect("minor"), 15_000);
        assert!(convert_money(usd_from_minor(100), CurrencyCode::GBP, &[]).is_err());
    }
    #[test]
    fn native_manifest_partial_identity_and_exhausted_versions_fail_closed() {
        let mut s = shipment();
        s.items[0].quantity = 1;
        assert!(TrackingPlan::new(&s, &source(), "in_transit").is_err());
        s = shipment();
        s.items[0].product_id = None;
        assert!(TrackingPlan::new(&s, &source(), "in_transit").is_err());
        s = shipment();
        s.order_id = OrderId::new();
        assert!(TrackingPlan::new(&s, &source(), "in_transit").is_err());
        s = shipment();
        s.version = i32::MAX;
        assert!(TrackingPlan::new(&s, &source(), "in_transit").is_err());
        s = shipment();
        assert!(TrackingPlan::new(&s, &source(), "dispatched").is_err());
        let mut source = source();
        source["sourceSystem"] = json!("sync");
        assert!(TrackingPlan::new(&s, &source, "in_transit").is_err());
    }
}
