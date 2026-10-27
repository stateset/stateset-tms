//! In-memory connection setup. No credential/key file or request is created here.
use crate::{
    config::{validate_origin, Config},
    model::clean,
    ui::Theme,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use reqwest::header;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub struct Wizard {
    pub fields: [String; 3],
    pub step: usize,
    pub error: Option<String>,
    original_origin: String,
    has_token: bool,
    has_key: bool,
    pub ready: bool,
    pub cancelled: bool,
}
impl Wizard {
    pub fn new(config: &Config, has_key: bool) -> Self {
        Self {
            fields: [config.origin.as_str().into(), String::new(), String::new()],
            step: 0,
            error: None,
            original_origin: config.origin.as_str().into(),
            has_token: config.headers.contains_key(header::AUTHORIZATION),
            has_key,
            ready: false,
            cancelled: false,
        }
    }
    fn validation(&self) -> Result<(), String> {
        let origin = validate_origin(self.fields[0].trim())?;
        if self.fields[1].is_empty() {
            if !self.has_token || origin.as_str() != self.original_origin {
                return Err("Enter a staff API credential for this API origin.".into());
            }
        } else {
            token_header(self.fields[1].trim())?;
        }
        let key = self.fields[2].trim();
        if !key.is_empty() && (key.len() != 64 || hex::decode(key).is_err()) {
            return Err("Recovery key must be the original 64 hexadecimal characters; leave blank to browse without one.".into());
        }
        Ok(())
    }
    pub fn paste(&mut self, value: &str) {
        if self.step >= 3 {
            return;
        }
        let value = value.trim();
        if !safe_input(value) {
            self.error = Some("Pasted text contains unsupported controls.".into());
            return;
        }
        let limit = [2048, 16384, 64][self.step];
        if self.fields[self.step].len().saturating_add(value.len()) > limit {
            self.error = Some("This field is too long; Ctrl+U clears it.".into());
            return;
        }
        self.fields[self.step].push_str(value);
        self.error = None;
    }
    /// true means explicit confirmation; paste and repeated Enter/y never confirm.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if key.code == KeyCode::Esc
            || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.cancelled = true;
            return false;
        }
        if self.step == 3 {
            if key.kind != KeyEventKind::Press {
                return false;
            }
            if key.code == KeyCode::BackTab || key.code == KeyCode::Backspace {
                self.step = 2;
            } else if key.code == KeyCode::Char('y') && self.ready {
                if let Err(error) = self.validation() {
                    self.error = Some(error);
                    return false;
                }
                return true;
            }
            return false;
        }
        match key.code {
            KeyCode::Backspace => {
                self.fields[self.step].pop();
                self.error = None;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.fields[self.step].clear();
                self.error = None;
            }
            KeyCode::BackTab if key.kind == KeyEventKind::Press => {
                self.step = self.step.saturating_sub(1);
                self.error = None;
            }
            KeyCode::Tab | KeyCode::Enter if key.kind == KeyEventKind::Press => {
                if self.step == 0 {
                    if let Err(error) = validate_origin(self.fields[0].trim()) {
                        self.error = Some(error);
                        return false;
                    }
                }
                if self.step == 1
                    && self.fields[1].is_empty()
                    && (!self.has_token || self.fields[0].trim() != self.original_origin)
                {
                    self.error = Some(
                        "Enter a staff API credential. It is never displayed or saved.".into(),
                    );
                    return false;
                }
                if self.step == 2 {
                    if let Err(error) = self.validation() {
                        self.error = Some(error);
                        return false;
                    }
                }
                self.step += 1;
                self.error = None;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && clean(&c.to_string()) == c.to_string()
                    && !c.is_control() =>
            {
                self.paste(&c.to_string())
            }
            _ => {}
        }
        false
    }
    pub fn apply(self, config: &mut Config) -> Result<(), String> {
        self.validation()?;
        config.origin = validate_origin(self.fields[0].trim())?;
        if !self.fields[1].is_empty() {
            config
                .headers
                .insert(header::AUTHORIZATION, token_header(self.fields[1].trim())?);
        }
        if !self.fields[2].is_empty() {
            config.recovery_key = Some(self.fields[2].trim().into());
        }
        config.connect = false;
        Ok(())
    }
    pub fn render(&mut self, frame: &mut Frame, theme: &Theme) {
        let area = frame.area();
        self.ready = false;
        frame.render_widget(
            Block::default().style(Style::default().fg(theme.foreground).bg(theme.background)),
            area,
        );
        if area.width < 52 || area.height < 18 {
            frame.render_widget(Paragraph::new("Connect to StateSet Freight\n\nResize to at least 52 columns × 18 rows.\nEsc cancels without connecting.").wrap(Wrap{trim:false}),area);
            return;
        }
        let width = area.width.saturating_sub(2).min(88);
        let height = area.height.saturating_sub(2);
        let rect = Rect::new(area.x + (area.width - width) / 2, area.y + 1, width, height);
        let masked = |index: usize, configured: bool| {
            if !self.fields[index].is_empty() {
                "•••••• (hidden)"
            } else if configured {
                "[configured; Enter keeps it]"
            } else {
                "[empty]"
            }
        };
        let mut lines = vec![
            "Connect to your freight workspace".into(),
            String::new(),
            "Credentials and recovery keys stay in memory. Esc cancels.".into(),
            String::new(),
            format!(
                "{} API origin: {}",
                if self.step == 0 { "›" } else { " " },
                clean(&self.fields[0])
            ),
            format!(
                "{} Staff credential: {}",
                if self.step == 1 { "›" } else { " " },
                masked(
                    1,
                    self.has_token && self.fields[0].trim() == self.original_origin
                )
            ),
            format!(
                "{} Recovery key: {}",
                if self.step == 2 { "›" } else { " " },
                masked(2, self.has_key)
            ),
            String::new(),
        ];
        if self.step < 3 {
            lines.extend([
                match self.step {
                    0 => "Use your HTTPS API origin. HTTP is supported on loopback.",
                    1 => "Paste a staff service credential or current Clerk JWT. Input is hidden.",
                    _ => {
                        "Paste the SAME 64-character recovery key on each launch. Input is hidden."
                    }
                }
                .into(),
                "A blank recovery key uses the configured key, or keeps saves disabled.".into(),
                "Keep the original recovery key in your approved secret storage.".into(),
                String::new(),
                "Enter / Tab next · Shift+Tab back · Ctrl+U clear field · Esc cancel".into(),
            ]);
        } else {
            lines.extend([
                "Review this API destination before connecting.".into(),
                format!(
                    "Save recovery: {}",
                    if !self.fields[2].is_empty() {
                        "supplied key; storage verified after sign-in"
                    } else if self.has_key {
                        "configured key; storage verified after sign-in"
                    } else {
                        "browsing only; no recovery key supplied"
                    }
                ),
                "No credential, key or read snapshot is saved by setup.".into(),
                String::new(),
                "y connects · Shift+Tab edits · Esc cancels".into(),
            ]);
        }
        if let Some(error) = &self.error {
            lines.push(String::new());
            lines.push(error.clone());
        }
        let widget = Paragraph::new(lines.join("\n"))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::bordered()
                    .borders(Borders::ALL)
                    .title(" Connection setup ")
                    .title_style(
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
            );
        self.ready = widget.line_count(rect.width) <= usize::from(rect.height);
        if self.ready {
            frame.render_widget(widget, rect);
        } else {
            frame.render_widget(Paragraph::new("Enlarge the terminal to see the full connection review.\nNo connection can be confirmed while it is hidden.\nEsc cancels.").wrap(Wrap{trim:false}),rect);
        }
    }
}
fn safe_input(value: &str) -> bool {
    value
        .chars()
        .all(|c| !c.is_control() && clean(&c.to_string()) == c.to_string())
}

fn token_header(token: &str) -> Result<header::HeaderValue, String> {
    if token.is_empty() || token.len() > 16384 || !safe_input(token) {
        return Err("Enter a valid staff API credential (at most 16 KiB).".into());
    }
    let mut value = header::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "Credential contains unsupported header characters.")?;
    value.set_sensitive(true);
    Ok(value)
}
pub fn run(
    terminal: &mut ratatui::DefaultTerminal,
    config: &mut Config,
    stop: &AtomicBool,
) -> Result<(), String> {
    let mut wizard = Wizard::new(
        config,
        std::env::var_os("FREIGHT_TUI_RECOVERY_KEY").is_some(),
    );
    let theme = Theme::new(config.no_color, config.light);
    loop {
        if stop.load(Ordering::Relaxed) || wizard.cancelled {
            return Err("Connection cancelled; no request was sent.".into());
        }
        terminal
            .draw(|f| wizard.render(f, &theme))
            .map_err(|_| "Cannot draw connection setup")?;
        if event::poll(Duration::from_millis(100)).map_err(|_| "Connection input failed")? {
            match event::read().map_err(|_| "Connection input failed")? {
                Event::Key(key) if wizard.key(key) => return wizard.apply(config),
                Event::Paste(value) => wizard.paste(&value),
                _ => {}
            }
        }
    }
}
