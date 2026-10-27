//! Session-local PostgreSQL budgets; migrations keep their separate local limits.
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

pub const DATABASE_BUSY: &str = "Database is temporarily busy. Retry the original request shortly.";
pub const DATABASE_UNAVAILABLE: &str =
    "Database is temporarily unavailable. Check the original request before retrying.";

#[derive(Clone, Copy)]
pub(crate) struct DatabaseBudget {
    pub(crate) acquire_ms: u64,
    pub(crate) statement_ms: u64,
    pub(crate) lock_ms: u64,
}
impl Default for DatabaseBudget {
    fn default() -> Self {
        Self {
            acquire_ms: 5000,
            statement_ms: 30000,
            lock_ms: 5000,
        }
    }
}
impl DatabaseBudget {
    fn parse(
        acquire: Option<&str>,
        statement: Option<&str>,
        lock: Option<&str>,
    ) -> Result<Self, sqlx::Error> {
        fn integer(raw: Option<&str>, default: u64, name: &str) -> Result<u64, sqlx::Error> {
            let Some(raw) = raw else { return Ok(default) };
            if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) {
                if let Ok(n @ 1..=300000) = raw.parse() {
                    return Ok(n);
                }
            }
            Err(sqlx::Error::Protocol(format!(
                "{name} must be an integer from 1 to 300000"
            )))
        }
        let defaults = Self::default();
        Ok(Self {
            acquire_ms: integer(
                acquire,
                defaults.acquire_ms,
                "FREIGHT_DATABASE_ACQUIRE_TIMEOUT_MS",
            )?,
            statement_ms: integer(
                statement,
                defaults.statement_ms,
                "FREIGHT_DATABASE_STATEMENT_TIMEOUT_MS",
            )?,
            lock_ms: integer(lock, defaults.lock_ms, "FREIGHT_DATABASE_LOCK_TIMEOUT_MS")?,
        })
    }
    pub(crate) fn from_env() -> Result<Self, sqlx::Error> {
        fn value(name: &str) -> Result<Option<String>, sqlx::Error> {
            match std::env::var(name) {
                Ok(v) => Ok(Some(v)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(_) => Err(sqlx::Error::Protocol(format!("Invalid {name}"))),
            }
        }
        Self::parse(
            value("FREIGHT_DATABASE_ACQUIRE_TIMEOUT_MS")?.as_deref(),
            value("FREIGHT_DATABASE_STATEMENT_TIMEOUT_MS")?.as_deref(),
            value("FREIGHT_DATABASE_LOCK_TIMEOUT_MS")?.as_deref(),
        )
    }
    pub(crate) fn pool_options(self, max: u32) -> PgPoolOptions {
        PgPoolOptions::new()
            .max_connections(max)
            .acquire_timeout(Duration::from_millis(self.acquire_ms))
            .after_connect(move |connection, _| Box::pin(async move {
                sqlx::query("SELECT set_config('statement_timeout',$1,false), set_config('lock_timeout',$2,false)")
                    .bind(format!("{}ms", self.statement_ms))
                    .bind(format!("{}ms", self.lock_ms))
                    .execute(connection).await?;
                Ok(())
            }))
    }
}

pub(crate) fn error(error: sqlx::Error) -> String {
    let unavailable = match &error {
        sqlx::Error::Io(_) | sqlx::Error::PoolClosed => true,
        // Explicit PostgreSQL connection/shutdown conditions. In particular,
        // 08P01 is a protocol violation, not an availability classification.
        sqlx::Error::Database(db) => db.code().is_some_and(|code| {
            matches!(
                code.as_ref(),
                "08000"
                    | "08001"
                    | "08003"
                    | "08004"
                    | "08006"
                    | "08007"
                    | "57P01"
                    | "57P02"
                    | "57P03"
                    | "57P05"
            )
        }),
        _ => false,
    };
    if unavailable {
        // Classification only: a disconnected write may already have committed.
        // Never retry it here or infer that its original receipt is absent.
        tracing::warn!(
            reason = "connection_unavailable",
            "freight_database_unavailable"
        );
        return DATABASE_UNAVAILABLE.into();
    }
    let reason = match &error {
        sqlx::Error::PoolTimedOut => Some("pool_timeout"),
        sqlx::Error::Database(db) => match db.code().as_deref() {
            Some("55P03") => Some("lock_timeout"),
            Some("57014") => Some("statement_cancelled"),
            _ => None,
        },
        _ => None,
    };
    if let Some(reason) = reason {
        tracing::warn!(reason, "freight_database_budget_exhausted");
        DATABASE_BUSY.into()
    } else {
        tracing::error!(%error, "Storage operation failed");
        "Internal storage error".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_loss_is_unavailable_without_masking_corruption_or_protocol_errors() {
        for kind in [
            std::io::ErrorKind::Other,
            std::io::ErrorKind::UnexpectedEof,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionRefused,
            std::io::ErrorKind::BrokenPipe,
        ] {
            let failure = sqlx::Error::Io(std::io::Error::new(kind, "private database details"));
            assert_eq!(error(failure), DATABASE_UNAVAILABLE);
        }
        assert_eq!(error(sqlx::Error::PoolClosed), DATABASE_UNAVAILABLE);
        assert_eq!(
            error(sqlx::Error::Protocol("private malformed response".into())),
            "Internal storage error"
        );
        assert_eq!(
            error(sqlx::Error::Decode(Box::new(std::io::Error::other(
                "invalid stored value"
            )))),
            "Internal storage error"
        );
    }
    #[test]
    fn database_budgets_are_finite_and_reject_ambiguous_configuration() {
        let d = DatabaseBudget::parse(None, None, None).expect("defaults");
        assert_eq!(
            (d.acquire_ms, d.statement_ms, d.lock_ms),
            (5000, 30000, 5000)
        );
        assert!(DatabaseBudget::parse(Some("1"), Some("300000"), Some("300000")).is_ok());
        for bad in [
            "",
            "0",
            "-1",
            "+2",
            " 2",
            "2 ",
            "1.5",
            "unlimited",
            "300001",
            "999999999999999999999",
        ] {
            for (args, name) in [
                (
                    (Some(bad), None, None),
                    "FREIGHT_DATABASE_ACQUIRE_TIMEOUT_MS",
                ),
                (
                    (None, Some(bad), None),
                    "FREIGHT_DATABASE_STATEMENT_TIMEOUT_MS",
                ),
                ((None, None, Some(bad)), "FREIGHT_DATABASE_LOCK_TIMEOUT_MS"),
            ] {
                let error = DatabaseBudget::parse(args.0, args.1, args.2)
                    .err()
                    .expect("invalid");
                assert!(error.to_string().contains(name));
            }
        }
    }
}
