#[cfg(test)]
mod action_tests;
mod actions;
#[cfg(test)]
mod catalog_tests;
mod client;
mod config;
mod demo;
#[cfg(test)]
mod ergonomics_tests;
mod evidence;
mod evidence_ui;
#[cfg(test)]
mod finance_tests;
mod graphics;
mod graphics_probe;
#[cfg(test)]
mod graphics_probe_tests;
#[cfg(test)]
mod graphics_tests;
mod journal;
mod model;
mod operations;
#[cfg(test)]
mod operations_tests;
mod pagination;
#[cfg(test)]
mod pagination_tests;
mod palette;
#[cfg(test)]
mod palette_tests;
mod parties;
#[cfg(test)]
mod party_tests;
#[cfg(test)]
mod queue_tests;
#[cfg(test)]
mod read_tests;
mod setup;
#[cfg(test)]
mod tests;
mod ui;
mod work_actions;
#[cfg(test)]
mod work_tests;
mod work_ui;

use config::Config;
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
};
use model::{clean, App};
use std::{
    collections::VecDeque,
    io::{self, IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

struct TerminalGuard(u32, Arc<AtomicBool>, graphics::Transport);
/// Owns the ratatui terminal. Its drop shows the cursor and reports failure
/// with `eprintln!`, which panics once the terminal has hung up, so a closed
/// terminal is released without running it; there is no screen to restore.
struct TerminalOwner(Option<ratatui::DefaultTerminal>);
impl Drop for TerminalOwner {
    fn drop(&mut self) {
        if let Some(terminal) = self.0.take() {
            if terminal_closed() {
                std::mem::forget(terminal);
            }
        }
    }
}
struct SynchronizedFrame(bool);
impl SynchronizedFrame {
    fn begin(enabled: bool) -> Result<Self, String> {
        if enabled {
            execute!(io::stdout(), crossterm::terminal::BeginSynchronizedUpdate)
                .map_err(|_| "Cannot begin terminal update")?;
        }
        Ok(Self(enabled))
    }
}
impl Drop for SynchronizedFrame {
    fn drop(&mut self) {
        if self.0 {
            let _ = execute!(io::stdout(), crossterm::terminal::EndSynchronizedUpdate);
        }
    }
}
enum Work {
    Read(client::Request),
    Write(Box<actions::WriteJob>),
    Evidence(evidence::Job),
}
enum Reply {
    Evidence(
        Box<evidence::Job>,
        Result<evidence::Content, client::Failure>,
    ),
    Read(
        u64,
        Result<client::Snapshot, client::Failure>,
        Option<journal::Status>,
    ),
    Write(bool, actions::Outcome, Option<journal::Status>),
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), crossterm::terminal::EndSynchronizedUpdate);
        if self.1.load(Ordering::Relaxed) {
            let _ = graphics::delete(&mut io::stdout(), self.0, true, self.2);
        }
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        restore_terminal();
    }
}
/// `ratatui::restore` reports failure with `eprintln!`, which panics when
/// stderr is the hung-up terminal; inside the panic hook that aborts.
fn restore_terminal() {
    if let Err(error) = ratatui::try_restore() {
        let _ = writeln!(io::stderr(), "Failed to restore terminal: {error}");
    }
}

/// Grace period for the event loop to honour `stop` before the process exits.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(1500);

/// True once the controlling terminal is gone. A live terminal with no input
/// reports nothing; a hung-up or closed one reports HUP, ERR or NVAL.
fn terminal_closed() -> bool {
    let stdin = io::stdin();
    let mut fds = [rustix::event::PollFd::new(
        &stdin,
        rustix::event::PollFlags::empty(),
    )];
    let zero = rustix::event::Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    match rustix::event::poll(&mut fds, Some(&zero)) {
        Ok(_) => fds[0].revents().intersects(
            rustix::event::PollFlags::HUP
                | rustix::event::PollFlags::ERR
                | rustix::event::PollFlags::NVAL,
        ),
        Err(_) => true,
    }
}

/// crossterm 0.29 retries end-of-file and read errors on a hung-up terminal
/// inside `event::poll`, so the event loop never regains control to observe
/// `stop` and ignores later SIGTERM/SIGHUP. Request shutdown on hangup, then
/// exit if the loop has not returned within the grace period.
fn spawn_shutdown_watchdog(stop: Arc<AtomicBool>) -> Result<(), String> {
    std::thread::Builder::new()
        .name("freight-tui-watchdog".into())
        .spawn(move || {
            let mut requested: Option<std::time::Instant> = None;
            loop {
                std::thread::sleep(Duration::from_millis(200));
                if terminal_closed() {
                    stop.store(true, Ordering::Relaxed);
                }
                if stop.load(Ordering::Relaxed) {
                    let since = *requested.get_or_insert_with(std::time::Instant::now);
                    if since.elapsed() >= SHUTDOWN_GRACE {
                        // The terminal may be gone; never panic writing to it.
                        let _ = execute!(io::stdout(), DisableBracketedPaste);
                        restore_terminal();
                        let _ = writeln!(io::stderr(), "freight-tui: Terminal closed");
                        std::process::exit(1);
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|_| "Cannot start the terminal shutdown watchdog".into())
}

fn main() {
    if let Err(error) = run() {
        // stderr may be the hung-up terminal; a failed write must not panic.
        let _ = writeln!(io::stderr(), "freight-tui: {}", clean(&error));
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let Some(mut config) = Config::parse(std::env::args().skip(1))? else {
        print!("{}", config::HELP);
        return Ok(());
    };
    if config.snapshot && config.needs_connection() {
        return Err("Set FREIGHT_TUI_TOKEN for snapshots, or run --connect in a terminal.".into());
    }
    if config.snapshot {
        let mut app = App {
            view: config.view,
            operations_scope: config.operations_scope,
            ..App::default()
        };
        let request = app.request();
        let snapshot = if config.demo {
            demo::snapshot(&request)
        } else {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Cannot initialize network runtime")?;
            runtime
                .block_on(client::fetch(&config.client()?, &config.origin, &request))
                .map_err(|e| e.message)?
        };
        app.apply(snapshot);
        println!(
            "StateSet Freight · {} · {}",
            if config.demo {
                "SYNTHETIC DEMO"
            } else {
                "API SNAPSHOT"
            },
            model::text(&app.identity["tenant_id"])
        );
        if app.view == model::View::Operations {
            println!("Operations: {}", app.operations_scope.title());
        }
        println!("{}", app.view.headers().join(" | "));
        for row in app.rows {
            println!("{}", row.cells.join(" | "));
        }
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("Interactive mode needs a terminal. Run inside Ghostty, or use --snapshot for plain output.".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register(signal, Arc::clone(&stop))
            .map_err(|_| "Cannot install terminal shutdown handler")?;
    }
    spawn_shutdown_watchdog(Arc::clone(&stop))?;
    let image_id = u32::from_be_bytes(
        uuid::Uuid::new_v4().as_bytes()[..4]
            .try_into()
            .map_err(|_| "Cannot allocate image ID")?,
    )
    .max(1);
    let transport = if config.graphics == graphics::Mode::Tmux {
        graphics::Transport::Tmux
    } else {
        graphics::Transport::Direct
    };
    let graphics_enabled = Arc::new(AtomicBool::new(
        matches!(
            config.graphics,
            graphics::Mode::Kitty | graphics::Mode::Tmux
        ) && (transport != graphics::Transport::Tmux || !config.no_color),
    ));
    let _guard = TerminalGuard(image_id, Arc::clone(&graphics_enabled), transport);
    let mut graphics = graphics::Renderer::new(image_id, transport);
    // Keep the hook in place before ratatui's: its hook restores with
    // `eprintln!`, which aborts as a nested panic on a hung-up terminal.
    let previous = std::panic::take_hook();
    let mut owner = TerminalOwner(Some(
        ratatui::try_init().map_err(|_| "Cannot initialize terminal")?,
    ));
    let terminal = owner.0.as_mut().ok_or("Cannot initialize terminal")?;
    execute!(io::stdout(), EnableBracketedPaste)
        .map_err(|_| "Cannot enable terminal paste handling")?;
    if transport == graphics::Transport::Tmux {
        // Placeholder placement uses the default underline color to select the
        // only virtual placement for its image. Reset inherited SGR state.
        execute!(
            io::stdout(),
            crossterm::style::SetAttribute(crossterm::style::Attribute::Reset)
        )
        .map_err(|_| "Cannot initialize terminal styles")?;
    }
    let panic_graphics = Arc::clone(&graphics_enabled);
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), crossterm::terminal::EndSynchronizedUpdate);
        if panic_graphics.load(Ordering::Relaxed) {
            let _ = graphics::delete(&mut io::stdout(), image_id, true, transport);
        }
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        restore_terminal();
        previous(info);
    }));
    if config.needs_connection() {
        setup::run(terminal, &mut config, &stop)?;
        terminal
            .clear()
            .map_err(|_| "Cannot clear connection setup")?;
    }
    let (sender, receiver) = mpsc::sync_channel::<Work>(1);
    let (results, replies) = mpsc::channel();
    let api = config.client()?;
    let origin = config.origin.clone();
    let demo = config.demo;
    let recovery_settings = journal::Settings::environment(config.recovery_key.take());
    std::thread::Builder::new()
        .name("freight-tui-api".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(v) => v,
                Err(_) => return,
            };
            let mut demo_state = demo::State::default();
            let mut journal = journal::Manager::new(recovery_settings, &origin);
            while let Ok(work) = receiver.recv() {
                let reply = match work {
                    Work::Evidence(job) => {
                        let result = if demo {
                            evidence::demo(&job)
                        } else {
                            runtime.block_on(evidence::fetch(&api, &origin, &job))
                        };
                        if result.as_ref().is_err_and(|error| error.denied) {
                            journal.close();
                        }
                        Reply::Evidence(Box::new(job), result)
                    }
                    Work::Read(request) => {
                        let result = if demo {
                            Ok(demo_state.snapshot(&request))
                        } else {
                            runtime.block_on(client::fetch(&api, &origin, &request))
                        };
                        let recovery = if demo {
                            None
                        } else {
                            match &result {
                                Ok(snapshot) => Some(journal.read(&snapshot.identity)),
                                Err(error) if error.denied => {
                                    journal.close();
                                    None
                                }
                                Err(_) => None,
                            }
                        };
                        Reply::Read(request.generation, result, recovery)
                    }
                    Work::Write(job) => {
                        let recovery = job.recovery();
                        let (outcome, status) = if demo {
                            (demo_state.execute(&job), None)
                        } else {
                            let (outcome, status) =
                                runtime.block_on(journal.execute(&api, &origin, &job));
                            (outcome, Some(status))
                        };
                        Reply::Write(recovery, outcome, status)
                    }
                };
                if results.send(reply).is_err() {
                    break;
                }
            }
        })
        .map_err(|_| "Cannot start the API worker")?;
    let theme = ui::Theme::new(config.no_color, config.light);
    let mut probe = graphics_probe::Probe::new(
        image_id,
        config.graphics,
        &std::env::var("TERM").unwrap_or_default(),
        std::env::var_os("TMUX").is_some()
            || std::env::var_os("ZELLIJ").is_some()
            || std::env::var_os("STY").is_some(),
        std::time::Instant::now(),
    );
    probe
        .send(&mut io::stdout())
        .map_err(|_| "Cannot query terminal graphics")?;
    let mut app = App {
        view: config.view,
        operations_scope: config.operations_scope,
        kitty_graphics: graphics_enabled.load(Ordering::Relaxed),
        kitty_image_id: (transport == graphics::Transport::Tmux).then_some(image_id),
        color_preview: !config.no_color,
        refresh_interval: config.refresh_interval,
        stale_after: config.stale_after,
        recovery_error: (!config.demo)
            .then(|| "Refresh to verify recovery storage before saving.".into()),
        ..App::default()
    };
    let mut evidence_busy = false;
    let mut pending_input = VecDeque::new();
    let mut graphics_viewport = terminal.size().map_err(|_| "Cannot read terminal size")?;
    while !app.quit && !stop.load(Ordering::Relaxed) {
        while let Ok(reply) = replies.try_recv() {
            match reply {
                Reply::Evidence(job, result) => {
                    evidence_busy = false;
                    app.finish_evidence(&job, result);
                }
                Reply::Read(generation, result, recovery) => {
                    let accepted = generation == app.generation && result.is_ok();
                    app.finish(generation, result);
                    if accepted {
                        if let Some(status) = recovery {
                            status.apply(&mut app);
                        }
                    }
                }
                Reply::Write(recovery, outcome, status) => {
                    app.finish_write(recovery, outcome);
                    if let Some(status) = status {
                        status.apply(&mut app);
                    }
                }
            }
        }
        let now = std::time::Instant::now();
        pending_input.extend(probe.tick(now));
        if let Some(event) = pending_input.pop_front() {
            handle_input(&mut app, event);
        }
        app.kitty_graphics =
            probe.enabled(now) && (transport != graphics::Transport::Tmux || !config.no_color);
        graphics_enabled.store(app.kitty_graphics, Ordering::Relaxed);
        app.tick(now);
        if app.loading.is_none() && !app.write_busy && !evidence_busy {
            if let Some(job) = app.write_ready.take() {
                sender
                    .try_send(Work::Write(Box::new(job)))
                    .map_err(|_| "API worker stopped")?;
                app.write_busy = true;
            }
        }
        if app.needs_fetch && app.loading.is_none() && !app.write_busy && !evidence_busy {
            sender
                .try_send(Work::Read(app.request()))
                .map_err(|_| "API worker stopped")?;
            app.loading = Some(app.generation);
            app.needs_fetch = false;
        }
        if !evidence_busy && app.loading.is_none() && !app.write_busy {
            if let Some(job) = app.evidence.as_mut().and_then(evidence::Panel::take_job) {
                sender
                    .try_send(Work::Evidence(job))
                    .map_err(|_| "API worker stopped")?;
                evidence_busy = true;
            }
        }
        if let Ok(size) = crossterm::terminal::window_size() {
            if size.columns > 0 && size.rows > 0 && size.width > 0 && size.height > 0 {
                app.image_cell = (
                    u32::from(size.width / size.columns).max(1),
                    u32::from(size.height / size.rows).max(1),
                );
            }
        }
        graphics.set_cell(app.image_cell);
        let synchronized_frame = SynchronizedFrame::begin(
            app.kitty_graphics && transport == graphics::Transport::Direct,
        )?;
        let viewport = terminal.size().map_err(|_| "Cannot read terminal size")?;
        if app.kitty_graphics && viewport != graphics_viewport {
            // Ratatui clears the screen when resizing. Ghostty can discard the
            // backing image during that clear, so reconstruct it after drawing.
            graphics
                .invalidate(&mut io::stdout())
                .map_err(|_| "Cannot reset preview")?;
            graphics_viewport = viewport;
        }
        let mut preview_error = false;
        terminal
            .draw(|frame| {
                ui::render(frame, &mut app, &theme, config.demo, config.origin.as_str());
                if app.kitty_graphics && transport == graphics::Transport::Tmux {
                    // Upload before Ratatui flushes placeholder text, avoiding
                    // visible missing-glyph cells on the initial frame.
                    let image = app
                        .evidence
                        .as_ref()
                        .and_then(|panel| panel.pixels.as_ref())
                        .zip(app.evidence_rect);
                    preview_error = graphics.update(&mut io::stdout(), image).is_err();
                }
            })
            .map_err(|_| "Terminal drawing failed")?;
        if preview_error {
            return Err("Preview drawing failed".into());
        }
        if app.kitty_graphics && transport == graphics::Transport::Direct {
            let image = app
                .evidence
                .as_ref()
                .and_then(|panel| panel.pixels.as_ref())
                .zip(app.evidence_rect);
            graphics
                .update(&mut io::stdout(), image)
                .map_err(|_| "Preview drawing failed")?;
        }
        drop(synchronized_frame);
        if pending_input.is_empty()
            && event::poll(Duration::from_millis(100)).map_err(|_| "Terminal input failed")?
        {
            let started = std::time::Instant::now();
            for _ in 0..512 {
                let input = event::read().map_err(|_| "Terminal input failed")?;
                let keys = probe.input(input, std::time::Instant::now());
                let ordinary_input = !keys.is_empty();
                pending_input.extend(keys);
                // Drain protocol bytes promptly, retaining the existing draw
                // between ordinary user inputs and reviewed action screens.
                if ordinary_input
                    || started.elapsed() >= Duration::from_millis(25)
                    || !event::poll(Duration::ZERO).map_err(|_| "Terminal input failed")?
                {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn handle_input(app: &mut App, input: Event) {
    match input {
        Event::Key(key) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
            app.key(key)
        }
        Event::Paste(value) => app.paste(&value),
        _ => {}
    }
}
