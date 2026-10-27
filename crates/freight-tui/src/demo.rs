use crate::{
    client::{Request, Snapshot},
    model::View,
};
use serde_json::{json, Value};

fn fixture() -> Snapshot {
    let identity = json!({"tenant_id":"synthetic-demo","features":{"operations_save_recovery":true},"access":{"subject":"demo:terminal","role":"operations","capabilities":["workspace.read","operations.write","saves.recover"]}});
    let lanes = [
        ("Los Angeles", "CA", "Phoenix", "AZ"),
        ("Dallas", "TX", "Atlanta", "GA"),
        ("Seattle", "WA", "Portland", "OR"),
        ("Chicago", "IL", "Detroit", "MI"),
    ];
    let statuses = [
        "in_transit",
        "planned",
        "accepted",
        "delivered",
        "tendered",
        "dispatched",
        "cancelled",
        "in_transit",
    ];
    let loads = (0..65).map(|i| {
        let (a,b,c,d) = lanes[i % lanes.len()];
        json!({"id":format!("load-demo-{i:04}"),"reference":format!("FRT-{:05}",10421+i),"status":statuses[i % statuses.len()],"customer_id":if i % 2 == 0 {"Acme Distribution (sample)"} else {"Northstar Supply (sample)"},"carrier_id":"Pioneer Transport (sample)","mode":"FTL","equipment":"Dry van · 53 ft","location":format!("{a}, {b}"),"origin":{"city":a,"state":b,"date":"2026-10-19"},"destination":{"city":c,"state":d,"date":"2026-10-20"},"exception":if i % 13 == 0 {json!("Pickup appointment requires follow-up")} else {Value::Null},"version":3,"updated_at":"2026-10-19T15:00:00Z"})
    }).collect::<Vec<_>>();
    let mut issues = [
        ("Late pickup follow-up", "Confirm the actual arrival with the carrier and facility.", 1),
        ("Delivery evidence needs review", "Obtain original POD and coordinate finance review.", 2),
        ("Outbound handoff needs recovery", "Inspect the original error and source holds before retrying.", 1)
    ].into_iter().enumerate().map(|(i,(title,next,priority))| json!({"id":format!("issue-demo-{i}"),"load_reference":loads[i]["reference"],"owner":if i==0 {json!("demo:dispatcher")} else {Value::Null},"source_active":true,"evidence_valid":true,"signal":{"load_id":loads[i]["id"],"title":title,"summary":"Synthetic work item for terminal exploration.","next_action":next,"priority":priority,"deadline":"2026-10-19T18:00:00Z"}})).collect::<Vec<_>>();
    for (index, issue) in issues.iter_mut().enumerate() {
        if index == 1 {
            issue["signal"]["summary"] =
                json!("Synthetic POD evidence work item for terminal exploration.");
        }
        issue["source_changed"] = json!(false);
        issue["deferred"] = json!(false);
        issue["follow_up_overdue"] = json!(false);
        if index == 2 {
            issue["owner"] = json!("demo:terminal");
        }
        issue["case"] = if issue["owner"].is_string() {
            json!({"status":"claimed","version":2,"follow_up_at":if index==0 {json!("2026-10-20T12:00:00Z")}else{Value::Null},"deferred_until":null})
        } else {
            Value::Null
        };
    }
    for (index, active, deferred, owner) in [
        (3, true, true, "demo:terminal"),
        (4, false, false, "demo:dispatcher"),
    ] {
        issues.push(json!({"id":format!("issue-demo-{index}"),"load_reference":loads[index]["reference"],"owner":owner,"source_active":active,"source_changed":false,"deferred":deferred,"follow_up_overdue":false,"evidence_valid":true,
            "case":{"status":"claimed","version":3,"follow_up_at":"2026-10-21T12:00:00Z","deferred_until":if deferred {json!("2026-10-21T12:00:00Z")}else{Value::Null}},
            "signal":{"load_id":loads[index]["id"],"title":if deferred {"Deferred carrier follow-up"}else{"Retained inactive source"},"summary":"Synthetic work item for queue exploration.","next_action":"Inspect retained work and current source conditions.","priority":1,"deadline":"2026-10-19T18:00:00Z"}}));
    }
    for (index, issue) in issues.iter_mut().enumerate() {
        let hash = format!("{index:064x}");
        issue["source_sha256"] = json!(hash);
        if !issue["case"].is_null() {
            issue["case"]["id"] = issue["id"].clone();
            issue["case"]["source_sha256"] = issue["source_sha256"].clone();
            issue["case"]["owner"] = issue["owner"].clone();
            issue["case"]["status"] = json!("working");
        }
        if issue["source_active"] == false {
            issue["source_changed"] = json!(true);
        }
    }
    let mut data = json!({"mode":"demo","tenant_id":"synthetic-demo","finance":{"receivable_cents":487500,"overdue_cents":300000,"prepared_payable_cents":340000,"carrier_credit_cents":12500},
        "bills":[{"id":"bill-demo-1","number":"INV-2026-1042","load_id":"load-demo-0003","customer_id":"Acme Distribution (sample)","status":"issued","version":2,"due_date":"2026-10-30","issued_at":"2026-10-18T12:00:00Z","snapshot":{"amount_cents":287500},"totals":{"balance_cents":187500,"received_cents":100000,"credited_cents":0,"overdue_days":0,"refund_due_cents":0}},
        {"id":"bill-demo-2","number":"INV-2026-1043","load_id":"load-demo-0011","customer_id":"Northstar Supply (sample)","status":"issued","version":1,"due_date":"2026-10-16","issued_at":"2026-10-10T12:00:00Z","snapshot":{"amount_cents":300000},"totals":{"balance_cents":300000,"received_cents":0,"credited_cents":0,"overdue_days":3,"refund_due_cents":0}}],
        "settlements":[{"id":"settle-demo-1","load_id":"load-demo-0003","carrier_id":"Pioneer Transport (sample)","status":"partially_reconciled","version":2,"total_amount_cents":240000,"balance_cents":140000,"credit_balance_cents":0,"created_at":"2026-10-18T12:00:00Z"},
        {"id":"settle-demo-2","load_id":"load-demo-0011","carrier_id":"Summit Freight (sample)","status":"prepared","version":1,"total_amount_cents":200000,"balance_cents":200000,"credit_balance_cents":0},
        {"id":"settle-demo-3","load_id":"load-demo-0019","carrier_id":"Pioneer Transport (sample)","status":"credit_due","version":3,"total_amount_cents":187500,"balance_cents":-12500,"credit_balance_cents":12500}],
        "outbox":[{"id":"out-demo-1","load_id":"load-demo-0001","destination":"stateset-edi","status":"retry","total_attempts":2,"transmission":"unknown","last_error":"Synthetic upstream timeout; inspect before retrying."},
        {"id":"out-demo-2","load_id":"load-demo-0003","destination":"stateset-sync","status":"sent","total_attempts":1,"transmission":"confirmed"}],
        "inbox":[{"id":"in-demo-1","document_id":"edi-demo-214","status":"applied","total_attempts":1}]});
    data["loads"] = json!(loads);
    data["issues"] = json!(issues);
    Snapshot { identity, data }
}

pub fn snapshot(request: &Request) -> Snapshot {
    State::default().snapshot(request)
}

#[derive(Default)]
pub struct State {
    loads: std::collections::BTreeMap<String, Value>,
    cases: std::collections::BTreeMap<String, Value>,
    receipts: std::collections::BTreeMap<String, (Vec<u8>, Value)>,
}
impl State {
    pub fn snapshot(&self, request: &Request) -> Snapshot {
        let mut snapshot = fixture();
        if let Some(loads) = snapshot.data["loads"].as_array_mut() {
            for load in loads {
                if let Some(updated) = load["id"].as_str().and_then(|id| self.loads.get(id)) {
                    *load = updated.clone();
                }
            }
        }
        if let Some(issues) = snapshot.data["issues"].as_array_mut() {
            for issue in issues {
                if let Some(case) = issue["id"].as_str().and_then(|id| self.cases.get(id)) {
                    issue["owner"] = case["owner"].clone();
                    issue["case"] = case.clone();
                    issue["deferred"] = json!(case["deferred_until"]
                        .as_str()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .is_some_and(|at| at > chrono::Utc::now()));
                }
            }
        }
        if request.view.paged() {
            let key = if request.view == View::Loads {
                "loads"
            } else {
                "issues"
            };
            let mut rows = snapshot.data[key].as_array().cloned().unwrap_or_default();
            let q = request.search.to_lowercase();
            rows.retain(|v| {
                (q.is_empty()
                    || if request.view == View::Operations {
                        format!(
                            "{} {} {}",
                            v["load_reference"].as_str().unwrap_or_default(),
                            v["signal"]["title"].as_str().unwrap_or_default(),
                            v["signal"]["summary"].as_str().unwrap_or_default()
                        )
                        .to_lowercase()
                        .contains(&q)
                    } else {
                        v.to_string().to_lowercase().contains(&q)
                    })
                    && (request.view != View::Loads
                        || request.status.is_empty()
                        || v["status"] == request.status)
                    && (request.view != View::Loads
                        || crate::parties::Party::ALL.into_iter().all(|party| {
                            request
                                .load_parties
                                .get(party)
                                .is_none_or(|id| v[party.field()] == id)
                        }))
                    && (request.view != View::Operations
                        || match request.operations_scope {
                            crate::operations::Scope::Actionable => {
                                v["source_active"] == true && v["deferred"] != true
                            }
                            crate::operations::Scope::Active => v["source_active"] == true,
                            crate::operations::Scope::Mine => {
                                v["source_active"] == true
                                    && v["owner"] == snapshot.identity["access"]["subject"]
                            }
                            crate::operations::Scope::Unowned => {
                                v["source_active"] == true && v["owner"].is_null()
                            }
                            crate::operations::Scope::Deferred => {
                                v["source_active"] == true && v["deferred"] == true
                            }
                            crate::operations::Scope::Inactive => v["source_active"] == false,
                            crate::operations::Scope::All => true,
                        })
            });
            let start = request
                .cursor
                .as_deref()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0)
                .min(rows.len());
            snapshot.data["next_cursor"] = if start + 50 < rows.len() {
                json!((start + 50).to_string())
            } else {
                Value::Null
            };
            snapshot.data[key] = json!(rows.into_iter().skip(start).take(50).collect::<Vec<_>>());
        }
        snapshot
    }
    pub fn execute(&mut self, job: &crate::actions::WriteJob) -> crate::actions::Outcome {
        use crate::actions::{Outcome, Owner, WriteJob};
        let pending = job.pending();
        let snapshot = fixture();
        if Owner::from_identity(&snapshot.identity).as_ref() != Some(&pending.owner) {
            return Outcome::Denied;
        }
        if let Some((body, result)) = self.receipts.get(&pending.key) {
            return if body != &pending.body {
                Outcome::Rejected("Original key/body conflict.".into())
            } else if result.is_null() {
                Outcome::Stopped
            } else {
                Outcome::Completed
            };
        }
        if let WriteJob::Stop { note, .. } = job {
            if note.trim().is_empty() || note.len() > 200 {
                return Outcome::Rejected("Resolution note required.".into());
            }
            self.receipts
                .insert(pending.key.clone(), (pending.body.clone(), Value::Null));
            return Outcome::Stopped;
        }
        if let crate::actions::Kind::Work(kind) = pending.kind {
            return self.execute_work(pending, kind, &snapshot);
        }
        let current = self.loads.get(&pending.target.id).or_else(|| {
            snapshot.data["loads"]
                .as_array()?
                .iter()
                .find(|v| v["id"] == pending.target.id)
        });
        let Some(current) = current else {
            return Outcome::Rejected("Shipment not found.".into());
        };
        if current["version"].as_u64() != Some(pending.target.version) {
            return Outcome::Rejected("Shipment changed; refresh and review.".into());
        }
        let Ok(body) = serde_json::from_slice::<Value>(&pending.body) else {
            return Outcome::Rejected("Invalid command.".into());
        };
        let mut load = current.clone();
        match pending.kind {
            crate::actions::Kind::Location => load["location"] = body["location"].clone(),
            crate::actions::Kind::Exception => load["exception"] = body["note"].clone(),
            crate::actions::Kind::Resolve => load["exception"] = Value::Null,
            crate::actions::Kind::Work(_) => {
                return Outcome::Rejected("Invalid shipment action.".into())
            }
        }
        load["version"] = json!(pending.target.version + 1);
        load["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
        self.receipts
            .insert(pending.key.clone(), (pending.body.clone(), load.clone()));
        self.loads.insert(pending.target.id.clone(), load);
        Outcome::Completed
    }
}

impl State {
    fn execute_work(
        &mut self,
        pending: &crate::actions::Pending,
        kind: crate::work_actions::WorkKind,
        snapshot: &Snapshot,
    ) -> crate::actions::Outcome {
        use crate::{
            actions::{Outcome, Target},
            work_actions::WorkKind,
        };
        let Some(mut row) = snapshot.data["issues"]
            .as_array()
            .and_then(|a| a.iter().find(|r| r["id"] == pending.target.id))
            .cloned()
        else {
            return Outcome::Rejected("Work not found.".into());
        };
        if let Some(case) = self.cases.get(&pending.target.id) {
            row["owner"] = case["owner"].clone();
            row["case"] = case.clone();
        }
        let Some(current) = Target::from_issue(&row) else {
            return Outcome::Rejected("Work evidence unavailable.".into());
        };
        if current != pending.target || !kind.allowed(&current, &pending.owner) {
            return Outcome::Rejected("Work changed; refresh and review.".into());
        }
        let Ok(body) = serde_json::from_slice::<Value>(&pending.body) else {
            return Outcome::Rejected("Invalid work action.".into());
        };
        if kind.timed() {
            let field = if kind == WorkKind::Defer {
                "defer_until"
            } else {
                "follow_up_at"
            };
            if kind
                .time(
                    body[field].as_str().unwrap_or_default(),
                    &current,
                    chrono::Utc::now(),
                )
                .is_err()
            {
                return Outcome::Rejected("Reviewed work time is no longer valid.".into());
            }
        }
        let mut case = row["case"].clone();
        if case.is_null() {
            case = json!({"id":pending.target.id,"version":1,"source_sha256":row["source_sha256"],"status":"open","owner":null,"follow_up_at":null,"deferred_until":null,"history":[{"action":"detected","actor":"system:operations","note":"Synthetic source detection","at":chrono::Utc::now().to_rfc3339()}]});
        }
        match kind {
            WorkKind::Claim => {
                case["owner"] = json!(pending.owner.subject);
                case["status"] = json!("working");
                case["follow_up_at"] = body["follow_up_at"].clone();
                case["deferred_until"] = Value::Null;
            }
            WorkKind::Release => {
                case["owner"] = Value::Null;
                case["status"] = json!("open");
                case["follow_up_at"] = Value::Null;
                case["deferred_until"] = Value::Null;
            }
            WorkKind::Note => {}
            WorkKind::FollowUp => {
                case["follow_up_at"] = body["follow_up_at"].clone();
                if case["deferred_until"]
                    .as_str()
                    .zip(body["follow_up_at"].as_str())
                    .is_some_and(|(a, b)| a > b)
                {
                    case["deferred_until"] = Value::Null;
                }
            }
            WorkKind::Defer => case["deferred_until"] = body["defer_until"].clone(),
            WorkKind::Resume => case["deferred_until"] = Value::Null,
        }
        case["version"] = json!(if current.version == 0 {
            2
        } else {
            current.version + 1
        });
        let mut history = case["history"].as_array().cloned().unwrap_or_default();
        history.push(json!({"action":kind.code(),"actor":pending.owner.subject,"note":body["note"],"at":chrono::Utc::now().to_rfc3339()}));
        case["history"] = json!(history);
        self.receipts
            .insert(pending.key.clone(), (pending.body.clone(), case.clone()));
        self.cases.insert(pending.target.id.clone(), case);
        Outcome::Completed
    }
}
