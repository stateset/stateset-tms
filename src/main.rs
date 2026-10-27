use stateset_freight::{
    api::App,
    auth::{credentials, Credential},
    integrations::{Integrations, WorkerConfig},
    lifecycle::{self, Shutdown, ShutdownConfig, Signals},
    store::Store,
};
use std::{sync::Arc, time::Duration};
mod pdf_sandbox;
mod readiness_probe;

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Serve,
    CheckReady,
    Migrate,
}
fn command(args: &[String]) -> Result<Command, &'static str> {
    match args {
        [] => Ok(Command::Serve),
        [flag] if flag == "--check-ready" => Ok(Command::CheckReady),
        [flag] if flag == "--migrate" => Ok(Command::Migrate),
        _ => Err("Usage: stateset-freight [--check-ready | --migrate]"),
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    // Enter the single-threaded PDF child before dotenv, credentials, listeners
    // or Tokio exist. The child restricts itself before exec/parsing any PDF.
    if args.first().map(String::as_str) == Some("--pdf-worker") {
        std::process::exit(i32::from(pdf_sandbox::entry(&args[1..], false)));
    }
    if args.first().map(String::as_str) == Some("--pdf-isolated-worker") {
        std::process::exit(i32::from(pdf_sandbox::entry(&args[1..], true)));
    }
    if args == ["--pdf-renderer-service"] {
        return stateset_freight::pdf_renderer::run_service();
    }
    if args == ["--check-pdf-renderer"] {
        return stateset_freight::pdf_renderer::check_service();
    }
    let command = command(&args)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        match command {
            Command::CheckReady => {
                dotenvy::dotenv().ok();
                let bind =
                    std::env::var("FREIGHT_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8090".into());
                readiness_probe::check(&bind).await
            }
            Command::Migrate => migrate().await,
            Command::Serve => run().await,
        }
    });
    // A cancelled request can still own an admitted blocking decode. Do not
    // wait without a bound for blocking jobs after the process drain finishes.
    runtime.shutdown_timeout(Duration::ZERO);
    result
}

async fn migrate() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let database = std::env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or("DATABASE_URL is required for --migrate")?;
    let mut signals = Signals::register()?;
    tokio::select! {
        biased;
        result = signals.recv() => {
            result?;
            Err("Schema migration cancelled before completion".into())
        }
        result = Store::migrate(&database) => {
            result?;
            println!("Freight schema migrated and verified");
            Ok(())
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stateset_freight=info,tower_http=info".into()),
        )
        .init();
    let demo = std::env::var("FREIGHT_DEMO").as_deref() == Ok("true");
    let pdf_renderer = Arc::new(stateset_freight::pdf_renderer::PdfRenderer::from_env()?);
    let identity = stateset_freight::identity::ClerkIdentity::from_env(demo)?;
    let mut keys =
        credentials(&std::env::var("FREIGHT_API_KEYS_JSON").unwrap_or_else(|_| "{}".into()))?;
    if demo {
        keys.insert("freight-local-demo".into(), Credential::from("demo"));
    }
    if keys.is_empty() && identity.is_none() {
        return Err("Set FREIGHT_API_KEYS_JSON or FREIGHT_CLERK_JSON, or explicitly enable FREIGHT_DEMO=true".into());
    }
    let worker_config = WorkerConfig::from_env()?;
    let shutdown_config = ShutdownConfig::from_env()?;
    let mut signals = Signals::register()?;
    let database = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty());
    if !demo && database.is_none() {
        return Err("DATABASE_URL is required outside demo mode".into());
    }
    let integrations = Integrations::from_env()?;
    let bind = std::env::var("FREIGHT_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8090".into());
    let startup = async {
        let store = Store::new(database.as_deref()).await?;
        if demo {
            store.seed_demo().await?;
            tracing::warn!("Demo credential enabled. Demo data is synthetic.");
        }
        let listener = tokio::net::TcpListener::bind(&bind).await?;
        Ok::<_, Box<dyn std::error::Error>>((store, listener))
    };
    let (store, listener) = tokio::select! {
        biased;
        result = signals.recv() => {
            result?;
            tracing::info!("Freight startup cancelled before services started");
            return Ok(());
        }
        result = startup => result?,
    };
    tracing::info!(%bind,durable=store.durable(),"StateSet Freight listening");
    lifecycle::serve(
        listener,
        App {
            pdf_renderer,
            identity: Arc::new(identity),
            store,
            keys: Arc::new(keys),
            demo,
            integrations: Arc::new(integrations),
        },
        worker_config,
        shutdown_config,
        Shutdown::default(),
        signals.recv(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_explicit_and_mutually_exclusive() {
        assert_eq!(command(&[]).expect("serve"), Command::Serve);
        assert_eq!(
            command(&["--check-ready".into()]).expect("probe"),
            Command::CheckReady
        );
        assert_eq!(
            command(&["--migrate".into()]).expect("migration"),
            Command::Migrate
        );
        for args in [
            vec!["--migrate".into(), "--check-ready".into()],
            vec!["--migrate".into(), "unexpected".into()],
            vec!["--migrate=true".into()],
        ] {
            assert!(command(&args).is_err());
        }
    }
}
