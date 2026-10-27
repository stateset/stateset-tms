//! Optional qualification against the sibling repository's actual X12 codec.
#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use stateset_edi_x12::{
        transactions::{
            freight210::parse_210,
            response990::parse_990,
            status214::parse_214,
            tender204::{try_build_204, Tender204},
        },
        x12::{parse_interchange, TransactionSet},
    };
    use stateset_freight::{
        domain::{demo, Status},
        integrations::normalize_document,
    };

    fn raw(set: &str, group: &str, body: &str) -> TransactionSet {
        let count = body.matches('~').count() + 2;
        let wire=format!("ISA*00*          *00*          *ZZ*CARRIER        *ZZ*STATESET       *261015*1200*U*00401*000000001*0*T*>~GS*{group}*CARRIER*STATESET*20261015*1200*1*X*004010~ST*{set}*0001~{body}SE*{count}*0001~GE*1*1~IEA*1*000000001~");
        parse_interchange(&wire)
            .expect("real codec must accept envelope")
            .groups[0]
            .transactions[0]
            .clone()
    }
    #[test]
    fn actual_gateway_codec_accepts_freight_generated_204() {
        let mut w = demo();
        w.apply(
            "load.command",
            "load_5",
            json!({"action":"tender","expected_version":1,"carrier_id":"ca2"}),
        )
        .expect("tender");
        let payload = &w.outbox[0].payload;
        let typed: Tender204 =
            serde_json::from_value(payload.clone()).expect("gateway Tender204 contract");
        assert!(!typed.orders.is_empty());
        let tx = try_build_204(&typed, "0001").expect("gateway codec accepts numeric constraints");
        assert_eq!(tx.id, "204");
        assert_eq!(typed.shipment_id, w.tenders[0].shipment_reference);
        assert_ne!(typed.shipment_id, "FR-1052");
        assert_eq!(typed.bol_number.as_deref(), Some("FR-1052"));
        let origin = typed.ship_from.as_ref().expect("origin");
        assert_eq!(origin.address_lines, vec!["100 Synthetic Demo Way"]);
        assert_eq!(origin.postal_code.as_deref(), Some("00000"));
        assert_eq!(origin.country.as_deref(), Some("US"));
        assert!(tx.segments.iter().any(
            |segment| segment.tag == "N3" && segment.elements == vec!["100 Synthetic Demo Way"]
        ));
        assert!(tx.segments.iter().any(|segment| segment.tag == "N4"
            && segment.get(3) == Some("00000")
            && segment.get(4) == Some("US")));
        assert_eq!(
            typed.ship_from.expect("origin").city.as_deref(),
            Some("Boise")
        );
    }
    #[test]
    fn actual_204_model_drops_intermediate_metadata_so_freight_requires_full_native_handoff() {
        let mut w = demo();
        let load = w.loads.iter_mut().find(|l| l.id == "load_5").expect("load");
        let mut stop = load.origin.clone();
        stop.facility = "INTERMEDIATE-CODEC-PROBE".into();
        load.intermediate_stops
            .push(stateset_freight::itinerary::IntermediateStop {
                id: "stop_codec".into(),
                kind: "delivery".into(),
                stop,
            });
        w.apply(
            "load.command",
            "load_5",
            json!({"action":"tender","expected_version":1,"carrier_id":"ca2"}),
        )
        .expect("native tender");
        let job = &w.outbox[0];
        assert_eq!(job.status, "native_only");
        assert_eq!(job.total_attempts, 0);
        let mut hypothetical = job.payload.clone();
        hypothetical["intermediate_stops"] = json!(
            w.loads
                .iter()
                .find(|l| l.id == "load_5")
                .expect("load")
                .intermediate_stops
        );
        let typed: Tender204 =
            serde_json::from_value(hypothetical).expect("gateway ignores unknown metadata");
        let wire = try_build_204(&typed, "0001").expect("actual builder");
        assert!(!wire.segments.iter().any(|s| s.tag == "S5"
            || s.elements
                .iter()
                .any(|e| e.contains("INTERMEDIATE-CODEC-PROBE"))));
        assert!(w.apply("outbox.retry", &job.id.clone(), json!({})).is_err());
    }
    #[test]
    fn accepted_customer_quote_books_then_builds_a_real_204_with_retained_selected_carrier_and_plan(
    ) {
        let mut w = demo();
        let plan = w
            .loads
            .iter()
            .find(|v| v.id == "load_8")
            .expect("fixture")
            .clone();
        let order = json!({"id":"00000000-0000-4000-8000-000000000777","orderNumber":"CODEC-SYNC-ORDER","status":"pending","grandTotal":2345});
        let quote_plan =
            serde_json::to_value(stateset_freight::quotes::QuotePlan::from_load(&plan))
                .expect("plan");
        let mut load = quote_plan.clone();
        load["reference"] = json!("CODEC-SOURCE-LOAD");
        load["buy_cents"] = json!(90000);
        load["sell_cents"] = json!(140000);
        let source = w
            .apply("sync.import", "", json!({"order":order,"load":load}))
            .expect("original import");
        let source_id = source["id"].as_str().expect("source");
        let load_count = w.loads.len();
        let until = chrono::Utc::now() + chrono::Duration::hours(12);
        let q=w.apply_quote("quote.create","",json!({"reference":"QUOTE-CODEC","planned_load_id":source_id,"expected_load_version":1,"plan":quote_plan,"sell_cents":150001,"valid_until":until,"terms":"Inclusive quoted shipment price"}),"ops:codec").expect("quote");
        let id = q["id"].as_str().expect("quote id");
        let c=w.apply_contract("contract.create","",json!({"number":"CODEC-CARRIER-LANE","carrier_id":"ca4","rate":{"customer_id":plan.customer_id,"origin":{"country":plan.origin.details.country,"state":plan.origin.state,"postal_code":plan.origin.details.postal_code},"destination":{"country":plan.destination.details.country,"state":plan.destination.state,"postal_code":plan.destination.details.postal_code},"equipment":plan.equipment,"max_weight_lbs":80000,"max_miles":10000,"effective_from":chrono::Utc::now().date_naive(),"effective_through":(chrono::Utc::now()+chrono::Duration::days(30)).date_naive(),"amount_cents":125001,"terms":"Inclusive carrier contract price"},"contact":"Carrier contracting lead","reference":"Original codec agreement","note":"Actual synthetic agreement inspected","reason":"Initial agreement"}),"ops:codec").expect("contract");
        let q=w.apply_quote("quote.contract-offer",id,json!({"expected_version":1,"agreement_id":c["id"],"expected_rate_version":1,"revision_sha256":c["revisions"][0]["basis_sha256"],"valid_until":chrono::Utc::now()+chrono::Duration::hours(24),"contact":"Carrier dispatch","reference":"REAL-CODEC-OFFER","note":"Actual current capacity confirmed at agreed price"}),"ops:codec").expect("offer");
        let offer = q["revisions"][0]["offers"][0]["id"]
            .as_str()
            .expect("offer");
        w.apply_quote(
            "quote.issue",
            id,
            json!({"expected_version":2,"offer_id":offer}),
            "ops:codec",
        )
        .expect("issue");
        w.apply_quote("quote.decision",id,json!({"expected_version":3,"decision":"accept","contact":"Customer transport lead","reference":"CUSTOMER-ACCEPTANCE","note":"Actual synthetic customer evidence"}),"ops:codec").expect("accept");
        let q = w
            .apply_quote(
                "quote.book",
                id,
                json!({"expected_version":4,"expected_load_version":1}),
                "ops:codec",
            )
            .expect("book");
        let load_id = q["booked_load_id"].as_str().expect("load");
        assert!(w.outbox.is_empty());
        assert_eq!(load_id, source_id);
        assert_eq!(w.loads.len(), load_count);
        assert_eq!(
            w.loads
                .iter()
                .find(|v| v.id == load_id)
                .expect("source retained")
                .source_order
                .as_ref(),
            Some(&order)
        );
        w.apply(
            "load.command",
            load_id,
            json!({"action":"tender","expected_version":2,"carrier_id":"ca4"}),
        )
        .expect("tender separately");
        let attempt = w
            .tenders
            .iter()
            .find(|v| v.load_id == load_id)
            .expect("tender");
        assert_eq!(
            (attempt.plan.buy_cents, attempt.plan.sell_cents),
            (125001, 150001)
        );
        let payload = &w
            .outbox
            .iter()
            .find(|v| v.load_id == load_id)
            .expect("204 handoff")
            .payload;
        let typed: Tender204 =
            serde_json::from_value(payload.clone()).expect("actual gateway contract");
        let tx = try_build_204(&typed, "0001").expect("real X12 codec");
        assert_eq!(typed.shipment_id, attempt.shipment_reference);
        assert_eq!(typed.carrier_scac, attempt.carrier.scac);
        assert_eq!(typed.orders[0].po_number, "CODEC-SYNC-ORDER");
        assert_eq!(tx.id, "204");
        let mut rate_workspace = w.clone();
        let confirmation=rate_workspace.apply_rate_confirmation("rate.issue",load_id,json!({"expected_load_version":3,"terms":"Actual inclusive carrier rate terms for this tender","reason":"Carrier-facing record accompanying the real 204"}),"ops:codec").expect("separate carrier rate record");
        assert_eq!(
            confirmation["snapshot"]["shipment_reference"],
            typed.shipment_id
        );
        assert_eq!(
            confirmation["snapshot"]["buy"]["total_amount_cents"],
            125001
        );
        let rate_html = confirmation["html"].as_str().expect("carrier document");
        assert!(rate_html.contains("1250.01"));
        assert!(!rate_html.contains("1500.01"));
        assert_eq!(
            rate_workspace
                .loads
                .iter()
                .find(|l| l.id == load_id)
                .expect("source retained")
                .source_order
                .as_ref(),
            Some(&order)
        );
        let publication = rate_workspace
            .apply_portal(
                "portal.publish",
                load_id,
                json!({"expected_load_version":4,"rate_confirmation_id":confirmation["id"]}),
                "ops:codec",
            )
            .expect("publish exact carrier offer");
        rate_workspace.apply_portal("portal.respond",publication["id"].as_str().expect("offer"),json!({"carrier_id":"ca4","subject":"carrier:codec","request":{"expected_load_version":5,"decision":"accept","reference":"Native carrier confirmation","note":"Carrier reviewed exact document accompanying this real 204","rate_confirmation_id":confirmation["id"],"rate_sha256":confirmation["sha256"]}}),"carrier:codec").expect("native carrier acceptance");
        let parsed = parse_990(&raw(
            "990",
            "GF",
            &format!("B1*BREX*{}*20261016*A~", typed.shipment_id),
        ))
        .expect("actual carrier 990");
        let note = json!({"document_id":"portal-following-990","partner_id":"brex","transaction_set":"990","direction":"inbound","status":"accepted"});
        let detail = json!({"document":{"id":"portal-following-990","partner_id":"brex","transaction_set":"990","direction":"inbound","status":"accepted","processed_at":chrono::Utc::now()},"parsed":parsed});
        let event = normalize_document(&note, &detail)
            .expect("normalize")
            .expect("supported 990");
        rate_workspace
            .apply("edi.event", "", event)
            .expect("EDI response after native acceptance");
        assert_eq!(
            rate_workspace
                .loads
                .iter()
                .find(|l| l.id == load_id)
                .expect("load")
                .status,
            Status::Accepted
        );
        assert_eq!(rate_workspace.portal_decisions.len(), 1);
        assert!(rate_workspace.journal.is_empty());
        let old_payload = payload.clone();
        let old_reference = typed.shipment_id.clone();
        w.apply(
            "load.command",
            load_id,
            json!({"action":"withdraw_tender","expected_version":3}),
        )
        .expect("withdraw original unsent tender");
        let mut amended_plan = quote_plan.clone();
        amended_plan["destination"]["city"] = json!("Tacoma");
        amended_plan["destination"]["state"] = json!("WA");
        amended_plan["destination"]["address_lines"] = json!(["99 Amended Dock", "Gate B"]);
        amended_plan["destination"]["postal_code"] = json!("98401");
        let parent_version = w
            .quotes
            .iter()
            .find(|q| q.id == id)
            .expect("parent")
            .version;
        let q=w.apply_quote("quote.amend",id,json!({"expected_version":parent_version,"expected_load_version":4,"reference":"QUOTE-CODEC-AMENDED","plan":amended_plan,"sell_cents":170001,"valid_until":until,"terms":"Inclusive amended customer price","reason":"Customer changed receiving dock before pickup"}),"ops:codec").expect("amendment proposal");
        let child = q["id"].as_str().expect("child");
        let q=w.apply_quote("quote.offer",child,json!({"expected_version":1,"carrier_id":"ca1","amount_cents":140001,"valid_until":chrono::Utc::now()+chrono::Duration::hours(24),"contact":"Replacement carrier dispatch","reference":"REAL-AMENDED-CARRIER-OFFER","note":"Actual amended inclusive agreement"}),"ops:codec").expect("amendment offer");
        let q = w
            .apply_quote(
                "quote.issue",
                child,
                json!({"expected_version":2,"offer_id":q["revisions"][0]["offers"][0]["id"]}),
                "ops:codec",
            )
            .expect("issue amendment");
        let digest = w
            .source_check(load_id)
            .expect("canonical source")
            .digest
            .clone();
        w.apply_quote("quote.decision",child,json!({"expected_version":q["version"],"decision":"accept","contact":"Actual customer transport lead","reference":"CUSTOMER-AMENDED-SOURCE-ACCEPTANCE","note":"Customer confirms amended plan and this canonical commerce order","source_digest":digest,"authorize_transport":false}),"ops:codec").expect("source-bound amendment acceptance");
        w.apply_quote(
            "quote.book",
            child,
            json!({"expected_version":4,"expected_load_version":4}),
            "ops:codec",
        )
        .expect("activate amendment");
        w.apply(
            "load.command",
            load_id,
            json!({"action":"tender","expected_version":5,"carrier_id":"ca1"}),
        )
        .expect("new carrier tender after amendment");
        let amended = w
            .outbox
            .iter()
            .rev()
            .find(|v| v.load_id == load_id && v.destination == "stateset-edi")
            .expect("amended handoff");
        let typed: Tender204 =
            serde_json::from_value(amended.payload.clone()).expect("actual amended Tender204");
        let tx = try_build_204(&typed, "0002").expect("amended plan passes real codec");
        assert_ne!(typed.shipment_id, old_reference);
        assert_eq!(typed.bol_number.as_deref(), Some("CODEC-SOURCE-LOAD"));
        assert_eq!(typed.orders[0].po_number, "CODEC-SYNC-ORDER");
        let to = typed.ship_to.expect("amended destination");
        assert_eq!(to.city.as_deref(), Some("Tacoma"));
        assert_eq!(to.address_lines, vec!["99 Amended Dock", "Gate B"]);
        assert!(tx
            .segments
            .iter()
            .any(|s| s.tag == "N3" && s.elements == vec!["99 Amended Dock", "Gate B"]));
        assert_eq!(
            w.outbox
                .iter()
                .find(|v| v.payload == old_payload)
                .expect("original handoff retained")
                .payload,
            old_payload
        );
        assert_eq!(w.loads.len(), load_count);
        assert_eq!(
            w.loads
                .iter()
                .find(|v| v.id == load_id)
                .expect("same shipment")
                .source_order
                .as_ref(),
            Some(&order)
        );
        assert_eq!(
            w.quotes
                .iter()
                .filter(|q| q.active_booking(load_id))
                .count(),
            1
        );
        assert!(w.journal.is_empty());
    }
    #[test]
    fn actual_990_codec_correlates_repeated_same_carrier_tenders_by_wire_identity() {
        let mut w = demo();
        w.apply(
            "load.command",
            "load_5",
            json!({"action":"tender","expected_version":1,"carrier_id":"ca2"}),
        )
        .expect("first");
        let first = w.tenders[0].shipment_reference.clone();
        w.apply(
            "load.command",
            "load_5",
            json!({"action":"withdraw_tender","expected_version":2}),
        )
        .expect("withdraw");
        w.apply(
            "load.command",
            "load_5",
            json!({"action":"tender","expected_version":3,"carrier_id":"ca2"}),
        )
        .expect("second");
        let second = w.tenders[1].shipment_reference.clone();
        for (index, reference) in [first, second].iter().enumerate() {
            let parsed = parse_990(&raw(
                "990",
                "GF",
                &format!("B1*PCLG*{reference}*20261015*A~"),
            ))
            .expect("real 990");
            let note = json!({"document_id":format!("response-{index}"),"partner_id":"pclg","transaction_set":"990","direction":"inbound","status":"accepted"});
            let detail = json!({"document":{"id":note["document_id"],"partner_id":"pclg","transaction_set":"990","direction":"inbound","status":"accepted","processed_at":chrono::Utc::now()},"parsed":parsed});
            let event = normalize_document(&note, &detail)
                .expect("normalize")
                .expect("event");
            let result = w.apply("edi.event", "", event).expect("qualified response");
            if index == 0 {
                assert_eq!(result["ignored"], true);
                assert_eq!(w.loads[4].status, Status::Tendered);
            } else {
                assert_eq!(w.loads[4].status, Status::Accepted);
            }
        }
    }
    #[test]
    fn actual_210_total_matches_only_documented_approved_additions_to_the_original_buy() {
        let mut w = demo();
        let charge=w.apply_charge_operation("charge.propose","load_1",json!({"expected_load_version":1,"side":"buy","kind":"lumper","description":"Agreed unloading service","amount_cents":7551,"evidence_reference":"Signed additional-service agreement"}),"operations:contract-test").expect("propose");
        w.apply_charge_operation("charge.review",charge["id"].as_str().expect("charge"),json!({"expected_load_version":2,"expected_version":1,"decision":"approved","counterparty_contact":"Carrier accounting","confirmation_reference":"External confirmation 15","note":"Inspected the actual service agreement"}),"finance:contract-test").expect("approve");
        let parsed = parse_210(&raw(
            "210",
            "IM",
            "B3**ADDITIONS-210*FR-1048*SMTT*PP*20261016*132551~C3*USD~",
        ))
        .expect("real 210");
        let note = json!({"document_id":"additional-210","partner_id":"smtt","transaction_set":"210","direction":"inbound","status":"accepted"});
        let detail = json!({"document":{"id":"additional-210","partner_id":"smtt","transaction_set":"210","direction":"inbound","status":"accepted","processed_at":chrono::Utc::now()},"parsed":parsed});
        let event = normalize_document(&note, &detail)
            .expect("normalize")
            .expect("supported");
        let result = w.apply("edi.event", "", event).expect("invoice");
        assert_eq!(result["amount_cents"], 132551);
        assert_eq!(result["variance_cents"], 0);
        assert_eq!(result["status"], "matched");
        assert_eq!(w.loads[0].buy_cents, 125000);
        assert_eq!(w.charges.len(), 1);
        assert!(
            w.journal.is_empty(),
            "Receiving a 210 does not approve a payable"
        );
    }
    #[test]
    fn actual_additional_210_stays_unposted_until_finance_classifies_the_supplement() {
        let mut w = demo();
        let invoice = w.invoices[0].clone();
        let load = w.loads[5].clone();
        let carrier = w
            .carriers
            .iter()
            .find(|v| Some(&v.id) == load.carrier_id.as_ref())
            .expect("carrier")
            .clone();
        w.apply("invoice.approve", &invoice.id, json!({}))
            .expect("primary approval");
        let parsed = parse_210(&raw(
            "210",
            "IM",
            &format!(
                "B3**SUP-REAL-210*{}*{}*PP*20261015*1251~C3*USD~",
                load.reference, carrier.scac
            ),
        ))
        .expect("actual 210");
        let note = json!({"document_id":"actual-supplement","partner_id":carrier.partner_id,"transaction_set":"210","direction":"inbound","status":"accepted"});
        let detail = json!({"document":{"id":"actual-supplement","partner_id":carrier.partner_id,"transaction_set":"210","direction":"inbound","status":"accepted","processed_at":chrono::Utc::now()},"parsed":parsed});
        let event = normalize_document(&note, &detail)
            .expect("normalize")
            .expect("supported");
        let result = w.apply("edi.event", "", event).expect("retained");
        assert_eq!(result["finance_review"], true);
        assert_eq!(w.journal.len(), 1);
        assert_eq!(w.invoices[0].amount_cents, 118000);
        let draft=w.apply_payable("payable.create",&load.id,json!({"expected_load_version":w.loads[5].version,"invoice_id":invoice.id,"kind":"supplemental_invoice","number":"SUP-REAL-210","document_amount_cents":1251,"issued_on":"2026-10-15","description":"Verified additional service","evidence_reference":"Canonical 210 and actual carrier agreement","carrier_document_id":result["carrier_document_id"]}),"finance:codec-test").expect("draft");
        w.apply_payable("payable.review",draft["id"].as_str().expect("id"),json!({"expected_load_version":w.loads[5].version,"expected_version":1,"decision":"approved","counterparty_contact":"Carrier accounting","confirmation_reference":"Actual external case","note":"Verified this is an additional claim, not a revised total"}),"finance:codec-test").expect("post");
        assert_eq!(w.carrier_invoice_total(&invoice.id).expect("total"), 119251);
        assert_eq!(w.invoices[0].amount_cents, 118000);
        assert_eq!(w.carrier_documents[0].status, "posted");
        assert_eq!(w.journal.len(), 2);
        let credit = parse_210(&raw(
            "210",
            "IM",
            &format!(
                "B3**CM-REAL-210*{}*{}*PP*20261015*-251~C3*USD~",
                load.reference, carrier.scac
            ),
        ))
        .expect("actual negative N2 210");
        let credit_note = json!({"document_id":"actual-credit","partner_id":carrier.partner_id,"transaction_set":"210","direction":"inbound","status":"accepted"});
        let detail = json!({"document":{"id":"actual-credit","partner_id":carrier.partner_id,"transaction_set":"210","direction":"inbound","status":"accepted","processed_at":chrono::Utc::now()},"parsed":credit});
        let event = normalize_document(&credit_note, &detail)
            .expect("normalize negative")
            .expect("supported");
        let mut before_primary = demo();
        before_primary.invoices.clear();
        let retained = before_primary
            .apply("edi.event", "", event.clone())
            .expect("negative document may precede the primary");
        assert_eq!(retained["finance_review"], true);
        assert!(before_primary.invoices.is_empty() && before_primary.journal.is_empty());
        assert_eq!(before_primary.carrier_documents[0].amount_cents, -251);
        let result = w
            .apply("edi.event", "", event)
            .expect("retain negative for finance");
        assert_eq!(result["finance_review"], true);
        assert_eq!(w.journal.len(), 2);
        let draft=w.apply_payable("payable.create",&load.id,json!({"expected_load_version":w.loads[5].version,"invoice_id":invoice.id,"kind":"credit_memo","number":"CM-REAL-210","document_amount_cents":251,"issued_on":"2026-10-15","description":"Actual carrier credit confirmed by finance","evidence_reference":"Canonical negative 210 plus carrier confirmation","carrier_document_id":result["carrier_document_id"]}),"finance:codec-test").expect("explicit credit classification");
        w.apply_payable("payable.review",draft["id"].as_str().expect("id"),json!({"expected_load_version":w.loads[5].version,"expected_version":1,"decision":"approved","counterparty_contact":"Carrier credit desk","confirmation_reference":"Actual credit memo confirmation","note":"Carrier explicitly confirmed this negative document is a credit memo"}),"finance:codec-test").expect("credit post");
        assert_eq!(
            w.carrier_invoice_total(&invoice.id)
                .expect("net after credit"),
            119000
        );
        assert_eq!(w.invoices[0].amount_cents, 118000);
    }
    #[test]
    fn actual_990_214_210_parsers_drive_freight_lifecycle_without_float_money() {
        let mut w = demo();
        let response = parse_990(&raw("990", "GF", "B1*ATFL*FR-1050*20261015*A~")).expect("990");
        let pickup = parse_214(&raw(
            "214",
            "QM",
            "B10*FR-1050**ATFL~LX*1~MS1*Denver*CO~AT7*AF*NS***20261015*1201~",
        ))
        .expect("214 pickup");
        let delivered = parse_214(&raw(
            "214",
            "QM",
            "B10*FR-1050**ATFL~LX*1~MS1*Phoenix*AZ~AT7*D1*NS***20261016*1202~",
        ))
        .expect("214 delivery");
        let invoice = parse_210(&raw(
            "210",
            "IM",
            "B3**FRT-123*FR-1050*ATFL*PP*20261016*275000~C3*USD~",
        ))
        .expect("210 invoice");
        let payloads = [
            ("990", serde_json::to_value(response).expect("json")),
            ("214", serde_json::to_value(pickup).expect("json")),
            ("214", serde_json::to_value(delivered).expect("json")),
            ("210", serde_json::to_value(invoice).expect("json")),
        ];
        for (i, (set, parsed)) in payloads.into_iter().enumerate() {
            let doc_id = format!("real-codec-{i}");
            let notification = json!({"document_id":doc_id,"partner_id":"atfl","transaction_set":set,"direction":"inbound","status":"accepted"});
            let detail = json!({"document":{"id":doc_id,"partner_id":"atfl","transaction_set":set,"direction":"inbound","status":"accepted","processed_at":format!("2026-10-15T12:0{i}:00Z")},"parsed":parsed});
            let event: Value = normalize_document(&notification, &detail)
                .expect("normalize")
                .expect("freight event");
            w.apply("edi.event", "", event)
                .expect("domain accepts actual codec data");
        }
        let shipment = w
            .loads
            .iter()
            .find(|v| v.reference == "FR-1050")
            .expect("shipment");
        assert_eq!(shipment.status, Status::Delivered);
        assert!(shipment.pod_reference.is_none());
        let invoice = w
            .invoices
            .iter()
            .find(|v| v.number == "FRT-123")
            .expect("invoice");
        assert_eq!(invoice.amount_cents, 275000);
        assert_eq!(invoice.variance_cents, 0);
    }
    #[test]
    fn combined_real_214_preserves_pickup_and_delivery_observations() {
        let parsed=parse_214(&raw("214","QM","B10*FR-1050**ATFL~LX*1~MS1*Denver*CO~AT7*AF*NS***20261015*1200~LX*2~MS1*Phoenix*AZ~AT7*D1*NS***20261016*1400~")).expect("combined 214");
        assert_eq!(parsed.events.len(), 2);
        let note = json!({"document_id":"combined","partner_id":"atfl","transaction_set":"214","direction":"inbound","status":"accepted"});
        let detail = json!({"document":{"id":"combined","partner_id":"atfl","transaction_set":"214","direction":"inbound","status":"accepted","processed_at":"2026-10-16T15:00:00Z"},"parsed":serde_json::to_value(parsed).expect("payload")});
        let mut workspace = demo();
        workspace
            .apply(
                "load.command",
                "load_3",
                json!({"action":"accept","expected_version":1}),
            )
            .expect("accepted");
        workspace
            .apply(
                "edi.event",
                "",
                normalize_document(&note, &detail)
                    .expect("normalize")
                    .expect("event"),
            )
            .expect("apply batch");
        assert_eq!(workspace.loads[2].status, Status::Delivered);
        assert!(workspace.loads[2].pod_reference.is_none());
        let observations: Vec<_> = workspace
            .events
            .iter()
            .filter(|v| v.kind == "tracking.observation")
            .collect();
        assert_eq!(observations.len(), 2);
        assert_eq!(
            observations[1].details.as_ref().expect("evidence")["observation"]["date"],
            "2026-10-16"
        );
    }
}
