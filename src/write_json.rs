//! Serialize a complete stored workspace without a temporary JSON value tree.
use crate::domain::Workspace;
use serde::Serialize;
use serde_json::value::RawValue;

pub(crate) fn encode(
    workspace: &Workspace,
    format: i32,
) -> Result<Box<RawValue>, serde_json::Error> {
    #[derive(Serialize)]
    struct Stored<'a> {
        #[serde(rename = "_freight_format")]
        format: i32,
        #[serde(flatten)]
        workspace: &'a Workspace,
    }
    serde_json::value::to_raw_value(&Stored { format, workspace })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{self, Receipt};
    use serde_json::{json, Value};

    #[test]
    fn complete_raw_encoding_preserves_legacy_models_receipts_and_quarantined_evidence() {
        let mut demo = domain::demo();
        demo.reconcile_operations(chrono::Utc::now());
        demo.receipts.insert("literal/\"雪{original}".into(), Receipt {
            fingerprint: "exact original fingerprint".into(),
            response: json!({"original":[null,true,-9223372036854775808i64,18446744073709551615u64,"snow 雪\nline",{"retained":-0.0}]}),
            stopped: None,
        });
        let mut sources = vec![json!(demo), json!(Workspace::default())];
        for retained in [
            json!(null),
            json!({"future":"retain"}),
            json!([{"future_case":{"original":[1,"two",null]}}]),
        ] {
            let mut value = json!(demo);
            value["operations_cases"] = retained;
            value["operations_policy"] =
                json!({"version":2,"tracking_stale_minutes":90,"future_policy":["retain exactly"]});
            sources.push(value);
        }
        for source in sources {
            let workspace: Workspace =
                serde_json::from_value(source).expect("supported or quarantined source");
            let mut legacy = json!(workspace);
            legacy["_freight_format"] = json!(18);
            let raw = encode(&workspace, 18).expect("raw JSON");
            assert_eq!(
                serde_json::from_str::<Value>(raw.get()).expect("JSON"),
                legacy
            );
            let decoded = crate::read_json::decode(raw.get(), 18).expect("typed stored source");
            assert_eq!(json!(decoded), json!(workspace));
        }
    }

    #[tokio::test]
    #[ignore = "requires disposable TEST_DATABASE_URL"]
    async fn postgres_raw_workspace_argument_retains_jsonb_type_and_exact_original_values() {
        let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
        let pool = sqlx::PgPool::connect(&url).await.expect("owned connection");
        let mut workspace = domain::demo();
        workspace.receipts.insert(
            "雪/original".into(),
            Receipt {
                fingerprint: "retained".into(),
                response: json!({"exact":[true,null,18446744073709551615u64]}),
                stopped: None,
            },
        );
        let mut legacy = json!(workspace);
        legacy["_freight_format"] = json!(18);
        let raw = encode(&workspace, 18).expect("encode");
        let (kind,equal,original):(String,bool,Value)=sqlx::query_as("SELECT pg_typeof($1)::text,$1=$2::jsonb,$1 #> ARRAY['receipts','雪/original','response']")
            .bind(sqlx::types::Json(raw)).bind(&legacy).fetch_one(&pool).await.expect("actual SQLx wire argument");
        assert_eq!(kind, "jsonb");
        assert!(equal);
        assert_eq!(original, workspace.receipts["雪/original"].response);
        pool.close().await;
    }
}
