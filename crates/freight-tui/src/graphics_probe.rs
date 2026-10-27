//! Nonblocking direct-transport negotiation. Terminal replies are not app keys.
use crate::graphics::Mode;
use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_millis(750);
const PREFIX_WAIT: Duration = Duration::from_millis(60);
const FRAME_WAIT: Duration = Duration::from_secs(1);
const MAX_REPLY: usize = 256;

pub struct Probe {
    id: u32,
    query: bool,
    deadline: Option<Instant>,
    enabled: bool,
    prefix: Vec<Event>,
    since: Option<Instant>,
    body: Option<String>,
    invalid: bool,
    escape: bool,
}

impl Probe {
    pub fn new(id: u32, mode: Mode, term: &str, multiplexed: bool, now: Instant) -> Self {
        let query = mode == Mode::Auto
            && term != "dumb"
            && !term.starts_with("tmux")
            && !term.starts_with("screen")
            && !multiplexed;
        Self {
            id,
            query,
            deadline: query.then_some(now + DEADLINE),
            enabled: matches!(mode, Mode::Kitty | Mode::Tmux),
            prefix: Vec::new(),
            since: None,
            body: None,
            invalid: false,
            escape: false,
        }
    }

    pub fn send(&self, out: &mut impl Write) -> io::Result<()> {
        if self.query {
            // Test precisely the zlib-compressed RGB direct transport we use.
            // a=q validates without storing or replacing any terminal image.
            let mut compressed =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            compressed.write_all(&[0, 0, 0])?;
            write!(
                out,
                "\x1b_Ga=q,t=d,f=24,o=z,s=1,v=1,i={};{}\x1b\\",
                self.id,
                STANDARD.encode(compressed.finish()?)
            )?;
            out.flush()?;
        }
        Ok(())
    }

    pub fn enabled(&mut self, now: Instant) -> bool {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
        }
        self.enabled
    }

    /// Flush an ordinary Esc/Alt-_ prefix promptly; cap malformed frame lifetime.
    pub fn tick(&mut self, now: Instant) -> Vec<Event> {
        self.enabled(now);
        if self.since.is_some_and(|at| {
            now.duration_since(at)
                >= if self.body.is_some() {
                    FRAME_WAIT
                } else {
                    PREFIX_WAIT
                }
        }) {
            self.since = None;
            if self.body.take().is_some() {
                // An unterminated terminal control frame cannot retain keyboard
                // capture indefinitely. Its buffered payload is never dispatched.
                self.invalid = false;
                self.escape = false;
            }
            return std::mem::take(&mut self.prefix);
        }
        Vec::new()
    }

    pub fn input(&mut self, event: Event, now: Instant) -> Vec<Event> {
        if !self.query {
            return vec![event];
        }
        // Crossterm exposes APC as ordinary key events. Keep the filter active
        // after timeout so a late complete reply cannot execute UI commands.
        if self.body.is_some() {
            if alt(&event, '\\') || (self.escape && plain(&event, '\\')) {
                let body = self.body.take().unwrap_or_default();
                if !self.invalid
                    && self.deadline.is_some_and(|deadline| now < deadline)
                    && body.starts_with(&format!("i={};", self.id))
                {
                    self.enabled = body == format!("i={};OK", self.id);
                    self.deadline = None;
                }
                self.since = None;
                self.invalid = false;
                self.escape = false;
            } else if matches!(&event, Event::Key(key) if key.code == KeyCode::Esc) {
                self.escape = true;
            } else if let Event::Key(key) = &event {
                if let KeyCode::Char(c) = key.code {
                    if c.is_ascii_graphic() || c == ' ' {
                        if !self.invalid
                            && !self.escape
                            && key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
                        {
                            let body = self.body.as_mut().expect("active reply");
                            if body.len() < MAX_REPLY {
                                body.push(c);
                            } else {
                                body.clear();
                                self.invalid = true;
                            }
                        } else {
                            self.invalid = true;
                        }
                    } else {
                        self.invalid = true;
                    }
                } else {
                    self.invalid = true;
                }
            } else {
                // Paste/resize/focus events are not terminal-response bytes.
                return vec![event];
            }
            return Vec::new();
        }
        if !self.prefix.is_empty() {
            let waiting_underscore = self.prefix.len() == 1
                && matches!(&self.prefix[0], Event::Key(key) if key.code == KeyCode::Esc);
            if waiting_underscore && plain(&event, '_') {
                self.prefix.push(event);
                return Vec::new();
            }
            if !waiting_underscore && plain(&event, 'G') {
                self.prefix.clear();
                self.body = Some(String::new());
                self.since = Some(now);
                return Vec::new();
            }
            let mut keys = std::mem::take(&mut self.prefix);
            self.since = None;
            keys.extend(self.input(event, now));
            return keys;
        }
        if alt(&event, '_') || matches!(&event, Event::Key(key) if key.code == KeyCode::Esc) {
            self.prefix.push(event);
            self.since = Some(now);
            return Vec::new();
        }
        vec![event]
    }
}

fn plain(event: &Event, c: char) -> bool {
    matches!(event, Event::Key(key) if key.code == KeyCode::Char(c) && key.kind != KeyEventKind::Release && key.modifiers.difference(KeyModifiers::SHIFT).is_empty())
}
fn alt(event: &Event, c: char) -> bool {
    matches!(event, Event::Key(key) if key.code == KeyCode::Char(c) && key.kind != KeyEventKind::Release && key.modifiers == KeyModifiers::ALT)
}
