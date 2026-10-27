use crate::{model::View, operations::Scope};
use reqwest::{header, Url};
use std::{env, time::Duration};

pub const HELP: &str = "StateSet Freight terminal console\n\nUsage: freight-tui [--demo | --connect] [--api ORIGIN] [--view NAME]\n                   [--refresh SECONDS] [--stale-after SECONDS]\n                   [--snapshot] [--no-color] [--light]\n\n  --demo          Explore synthetic freight without a server or credential\n  --connect       Guided live connection with hidden credential/key fields\n  --api           API origin (FREIGHT_TUI_API_URL or loopback port 8090)\n  --view          loads, operations, billing, payables, integrations\n  --operations-scope  Operations queue: actionable (default), active, mine,\n                      unowned, deferred, resolved (source inactive), all\n  --refresh       Automatic read interval: 5–3600 seconds; 0/off by default\n  --stale-after   Mark old snapshots STALE after 10–3600 seconds (default 60)\n  --snapshot      Print the selected view and exit; suitable for a pipe\n  --no-color      Disable colors (also honors NO_COLOR)\n  --graphics      auto (query direct support), kitty (force direct),\n                  kitty-tmux (quiet passthrough + placeholders), off\n  --light         Use a light background\n\nSet FREIGHT_TUI_TOKEN, or enter it through guided setup in a terminal.\nFREIGHT_TUI_ORGANIZATION_ID optionally selects a Clerk organization.\nLive saves need the original FREIGHT_TUI_RECOVERY_KEY (64 hex characters),\nor the same key entered through setup. Keep it in approved secret storage.\nFREIGHT_TUI_RECOVERY_DIR optionally selects an absolute private directory.\nSetup never saves or prints credentials/keys; browsing works without a key.\nAutomatic reads pause for editing, inspection, recovery and connection errors.\n\nKeys: 1-5 / Tab views, arrows / j k select, / search, s status, n / p pages,\n      r first page, Ctrl+R reread page, R automatic reads,\n      a actions, c / x original recovery,\n      f work/balance/queue filter, Ctrl+P commands,\n      v shipment evidence, Enter details, i connection/recovery status, ? help, q quit.\n";

pub struct Config {
    pub graphics: crate::graphics::Mode,
    pub connect: bool,
    pub recovery_key: Option<String>,
    pub refresh_interval: Option<Duration>,
    pub stale_after: Duration,
    pub demo: bool,
    pub snapshot: bool,
    pub no_color: bool,
    pub light: bool,
    pub view: View,
    pub operations_scope: Scope,
    pub origin: Url,
    pub headers: header::HeaderMap,
}
impl Config {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut args = args.into_iter();
        let mut graphics = crate::graphics::Mode::Auto;
        let mut connect = false;
        let mut refresh_interval = None;
        let mut stale_after = Duration::from_secs(60);
        let mut demo = false;
        let mut snapshot = false;
        let mut no_color = env::var_os("NO_COLOR").is_some();
        let mut light = false;
        let mut view = View::Loads;
        let mut operations_scope = None;
        let mut api =
            env::var("FREIGHT_TUI_API_URL").unwrap_or_else(|_| "http://127.0.0.1:8090".into());
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--help" | "-h" => return Ok(None),
                "--demo" => demo = true,
                "--graphics" => {
                    graphics = match args.next().as_deref() {
                        Some("auto") => crate::graphics::Mode::Auto,
                        Some("kitty") => crate::graphics::Mode::Kitty,
                        Some("kitty-tmux") => crate::graphics::Mode::Tmux,
                        Some("off") => crate::graphics::Mode::Off,
                        _ => {
                            return Err("--graphics requires auto, kitty, kitty-tmux, or off".into())
                        }
                    }
                }
                "--connect" => connect = true,
                "--refresh" => {
                    let seconds = args
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .ok_or("--refresh requires 0 (off), or 5–3600 seconds")?;
                    if seconds != 0 && !(5..=3600).contains(&seconds) {
                        return Err("--refresh requires 0 (off), or 5–3600 seconds".into());
                    }
                    refresh_interval = (seconds != 0).then(|| Duration::from_secs(seconds));
                }
                "--stale-after" => {
                    let seconds = args
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .filter(|s| (10..=3600).contains(s))
                        .ok_or("--stale-after requires 10–3600 seconds")?;
                    stale_after = Duration::from_secs(seconds);
                }
                "--snapshot" => snapshot = true,
                "--no-color" => no_color = true,
                "--light" => light = true,
                "--view" => {
                    view = match args.next().as_deref() {
                        Some("loads") => View::Loads,
                        Some("operations") => View::Operations,
                        Some("billing") => View::Billing,
                        Some("payables") => View::Payables,
                        Some("integrations") => View::Integrations,
                        _ => return Err(
                            "--view requires loads, operations, billing, payables, or integrations"
                                .into(),
                        ),
                    }
                }
                "--operations-scope" => {
                    operations_scope = Some(args.next().and_then(|v| Scope::from_code(&v)).ok_or("--operations-scope requires actionable, active, mine, unowned, deferred, resolved, or all")?);
                }
                "--api" => api = args.next().ok_or("--api requires an origin")?,
                _ => return Err("Unknown argument. Run freight-tui --help.".into()),
            }
        }
        if operations_scope.is_some() && view != View::Operations {
            return Err("--operations-scope requires --view operations".into());
        }
        if connect && (demo || snapshot) {
            return Err(
                "--connect is for interactive live access; use it without --demo or --snapshot."
                    .into(),
            );
        }
        let origin = validate_origin(&api)?;
        let mut headers = header::HeaderMap::new();
        if !demo {
            if let Ok(token) = env::var("FREIGHT_TUI_TOKEN") {
                if token.is_empty() || token.len() > 16 * 1024 {
                    return Err("FREIGHT_TUI_TOKEN must contain a valid staff credential.".into());
                }
                let mut value = header::HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| "Invalid credential header")?;
                value.set_sensitive(true);
                headers.insert(header::AUTHORIZATION, value);
            }
            if let Ok(org) = env::var("FREIGHT_TUI_ORGANIZATION_ID") {
                headers.insert(
                    "x-freight-organization-id",
                    header::HeaderValue::from_str(&org).map_err(|_| "Invalid organization ID")?,
                );
            }
        }
        Ok(Some(Self {
            graphics,
            connect,
            recovery_key: None,
            refresh_interval,
            stale_after,
            demo,
            snapshot,
            no_color,
            light,
            view,
            operations_scope: operations_scope.unwrap_or_default(),
            origin,
            headers,
        }))
    }
    pub fn needs_connection(&self) -> bool {
        !self.demo && (self.connect || !self.headers.contains_key(header::AUTHORIZATION))
    }
    pub fn client(&self) -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .default_headers(self.headers.clone())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("stateset-freight-tui/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| "Cannot initialize the API client".into())
    }
}
pub fn validate_origin(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| "Invalid API origin")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.host_str().is_none()
        || !(url.scheme() == "https" || url.scheme() == "http" && local)
    {
        return Err("Use an HTTPS API origin without a path or credentials; HTTP is allowed only on loopback.".into());
    }
    Ok(url)
}
