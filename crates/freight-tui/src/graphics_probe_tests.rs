use crate::{graphics::Mode, graphics_probe::Probe};
use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::{
    io::Read,
    time::{Duration, Instant},
};

fn key(c: char) -> Event {
    Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        if c.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        },
    ))
}
fn alt(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
}
fn esc() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
}
fn reply(probe: &mut Probe, body: &str, now: Instant, fragmented: bool) -> Vec<Event> {
    let mut events = if fragmented {
        vec![esc(), key('_'), key('G')]
    } else {
        vec![alt('_'), key('G')]
    };
    events.extend(body.chars().map(key));
    events.extend(if fragmented {
        vec![esc(), key('\\')]
    } else {
        vec![alt('\\')]
    });
    events
        .into_iter()
        .flat_map(|e| probe.input(e, now))
        .collect()
}
#[test]
fn query_tests_exact_compressed_transport_without_storing_or_placing_an_image() {
    let now = Instant::now();
    let probe = Probe::new(41072, Mode::Auto, "xterm-256color", false, now);
    let mut out = Vec::new();
    probe.send(&mut out).expect("query");
    let wire = String::from_utf8(out).expect("ascii");
    assert!(wire.starts_with("\x1b_Ga=q,t=d,f=24,o=z,s=1,v=1,i=41072;"));
    let payload = wire
        .split_once(';')
        .expect("payload")
        .1
        .strip_suffix("\x1b\\")
        .expect("terminator");
    let bytes = STANDARD.decode(payload).expect("base64");
    let mut rgb = Vec::new();
    flate2::read::ZlibDecoder::new(bytes.as_slice())
        .read_to_end(&mut rgb)
        .expect("zlib");
    assert_eq!(rgb, [0, 0, 0]);
    assert!(!wire.contains("a=t") && !wire.contains("a=p"));
}
#[test]
fn only_correlated_success_before_deadline_enables_auto_including_unknown_term_names() {
    let now = Instant::now();
    for fragmented in [false, true] {
        let mut probe = Probe::new(41, Mode::Auto, "other-terminal", false, now);
        assert!(!probe.enabled(now));
        assert!(reply(&mut probe, "i=42;OK", now, fragmented).is_empty());
        assert!(!probe.enabled(now));
        assert!(reply(&mut probe, "i=41;OK", now, fragmented).is_empty());
        assert!(probe.enabled(now));
        assert!(reply(&mut probe, "i=41;EINVAL: r q a c", now, fragmented).is_empty());
        assert!(probe.enabled(now));
    }
}
#[test]
fn negative_missing_malformed_and_late_replies_stay_fallback_without_dispatching_payload_keys() {
    let now = Instant::now();
    for body in [
        "i=41;ENOTSUP:r q a c",
        "i=41;OKextra",
        "i=041;OK",
        "i=41,i=41;OK",
        "i=42;OK",
        "q",
    ] {
        let mut probe = Probe::new(41, Mode::Auto, "xterm-ghostty", false, now);
        assert!(reply(&mut probe, body, now, false).is_empty());
        assert!(!probe.enabled(now));
    }
    let mut probe = Probe::new(41, Mode::Auto, "xterm-kitty", false, now);
    let late = now + Duration::from_secs(2);
    assert!(!probe.enabled(late));
    assert!(reply(&mut probe, "i=41;OK", late, true).is_empty());
    assert!(!probe.enabled(late));
    assert_eq!(probe.input(key('v'), late), vec![key('v')]);
}
#[test]
fn off_force_dumb_and_multiplexers_never_probe_or_capture_input() {
    let now = Instant::now();
    for (mode, term, mux, enabled) in [
        (Mode::Off, "xterm-ghostty", false, false),
        (Mode::Kitty, "unknown", true, true),
        (Mode::Tmux, "tmux-256color", true, true),
        (Mode::Auto, "dumb", false, false),
        (Mode::Auto, "xterm-ghostty", true, false),
        (Mode::Auto, "tmux-256color", false, false),
        (Mode::Auto, "screen-256color", false, false),
    ] {
        let mut probe = Probe::new(41, mode, term, mux, now);
        let mut out = Vec::new();
        probe.send(&mut out).expect("disabled query");
        assert!(out.is_empty());
        assert_eq!(probe.enabled(now), enabled);
        assert_eq!(probe.input(esc(), now), vec![esc()]);
    }
}
#[test]
fn ordinary_navigation_pastes_resize_and_alt_keys_remain_in_order() {
    let now = Instant::now();
    let mut probe = Probe::new(41, Mode::Auto, "xterm", false, now);
    for event in [
        key('v'),
        Event::Paste("r qa\nUnicode 日本".into()),
        Event::Resize(80, 24),
        alt('x'),
    ] {
        assert_eq!(probe.input(event.clone(), now), vec![event]);
    }
    assert!(probe.input(esc(), now).is_empty());
    assert_eq!(probe.input(key('v'), now), vec![esc(), key('v')]);
    assert!(probe.input(alt('_'), now).is_empty());
    assert_eq!(probe.input(key('x'), now), vec![alt('_'), key('x')]);
    assert!(probe.input(esc(), now).is_empty());
    assert!(probe.input(key('_'), now).is_empty());
    assert_eq!(probe.input(key('x'), now), vec![esc(), key('_'), key('x')]);
    assert!(probe.input(esc(), now).is_empty());
    assert!(probe.tick(now + Duration::from_millis(59)).is_empty());
    assert_eq!(probe.tick(now + Duration::from_millis(60)), vec![esc()]);
}
#[test]
fn oversized_and_truncated_frames_are_bounded_and_cannot_keep_keyboard_capture() {
    let now = Instant::now();
    let mut probe = Probe::new(41, Mode::Auto, "xterm", false, now);
    assert!(reply(
        &mut probe,
        &format!("i=41;OK{}", "q".repeat(2000)),
        now,
        false
    )
    .is_empty());
    assert!(!probe.enabled(now));
    for event in [alt('_'), key('G'), key('q')] {
        assert!(probe.input(event, now).is_empty());
    }
    let later = now + Duration::from_secs(1);
    assert!(probe.tick(later).is_empty());
    assert_eq!(probe.input(key('q'), later), vec![key('q')]);
    assert!(!probe.enabled(later));
}
