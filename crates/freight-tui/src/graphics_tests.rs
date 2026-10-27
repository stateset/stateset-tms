use crate::{config::Config, demo, evidence, graphics, model::App, ui};
use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};

fn app() -> App {
    let mut app = App {
        needs_fetch: false,
        ..App::default()
    };
    app.apply(demo::snapshot(&app.request()));
    app
}
fn key(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}
fn loaded() -> App {
    let mut app = app();
    key(&mut app, KeyCode::Char('v'));
    for _ in 0..2 {
        let job = app
            .evidence
            .as_mut()
            .expect("panel")
            .take_job()
            .expect("job");
        let result = evidence::demo(&job);
        app.finish_evidence(&job, result);
    }
    app
}
#[test]
fn transport_chunks_losslessly_and_repositions_without_retransmission() {
    let app = loaded();
    let pixels = app
        .evidence
        .as_ref()
        .expect("panel")
        .pixels
        .as_ref()
        .expect("pixels");
    let mut renderer = graphics::Renderer::new(710001, graphics::Transport::Direct);
    let mut out = Vec::new();
    let area = Rect::new(3, 5, 20, 18);
    renderer
        .update(&mut out, Some((pixels, area)))
        .expect("transmit");
    let raw = String::from_utf8(out.clone()).expect("ascii transport");
    let mut decoded = Vec::new();
    for command in raw.split("\x1b_G").skip(1) {
        let (header, rest) = command.split_once(';').expect("command");
        if header.contains("a=p") {
            continue;
        }
        let payload = rest.split("\x1b\\").next().expect("payload");
        assert!(payload.len() <= 4096);
        assert_eq!(payload.len() % 4, 0);
        decoded.extend(STANDARD.decode(payload).expect("base64"));
    }
    let mut restored = Vec::new();
    std::io::Read::read_to_end(
        &mut flate2::read::ZlibDecoder::new(decoded.as_slice()),
        &mut restored,
    )
    .expect("lossless zlib");
    assert_eq!(restored, pixels.rgb);
    assert!(raw.contains("a=t,t=d,f=24"));
    assert!(raw.contains("\x1b7\x1b[6;4H"));
    assert!(raw.contains("C=1,q=2;"));
    assert!(!raw.contains("t=f") && !raw.contains("t=s"));
    out.clear();
    renderer
        .update(&mut out, Some((pixels, area)))
        .expect("unchanged");
    assert!(out.is_empty());
    renderer
        .update(&mut out, Some((pixels, Rect::new(1, 1, 10, 9))))
        .expect("resize");
    let move_only = String::from_utf8(out.clone()).expect("ascii");
    assert!(move_only.contains("d=i,i=710001"));
    assert!(!move_only.contains("a=t"));
    out.clear();
    renderer.invalidate(&mut out).expect("viewport clear");
    assert!(String::from_utf8(out.clone())
        .expect("ascii")
        .contains("d=I,i=710001"));
    out.clear();
    renderer
        .update(&mut out, Some((pixels, area)))
        .expect("restore cleared terminal image");
    assert!(String::from_utf8(out.clone())
        .expect("ascii")
        .contains("a=t,"));
    out.clear();
    renderer.update(&mut out, None).expect("close");
    assert_eq!(
        String::from_utf8(out).expect("ascii"),
        "\x1b_Ga=d,d=I,i=710001,q=2;\x1b\\"
    );
}
#[test]
fn modes_are_explicit_and_reject_unknown_options() {
    for mode in ["auto", "kitty", "kitty-tmux", "off"] {
        assert!(Config::parse(["--demo", "--graphics", mode].map(String::from)).is_ok());
    }
    assert!(Config::parse(["--graphics", "wrong"].map(String::from)).is_err());
}
#[test]
fn closing_reopening_and_denial_cannot_resurrect_private_evidence() {
    let mut app = app();
    key(&mut app, KeyCode::Char('v'));
    let old = app
        .evidence
        .as_mut()
        .expect("panel")
        .take_job()
        .expect("job");
    key(&mut app, KeyCode::Esc);
    assert!(app.evidence.is_none());
    key(&mut app, KeyCode::Char('v'));
    app.finish_evidence(&old, evidence::demo(&old));
    assert!(app
        .evidence
        .as_ref()
        .expect("new panel")
        .documents
        .is_empty());
    app.finish_evidence(
        &old,
        Err(crate::client::Failure {
            message: "denied".into(),
            denied: true,
        }),
    );
    assert!(app.evidence.is_none());
    assert!(app.rows.is_empty());
    assert!(app.identity.is_null());
}
#[test]
fn viewer_pauses_reads_has_text_fallback_and_cleans_placement_on_tiny_resize() {
    let mut app = loaded();
    assert_eq!(app.refresh_pause(), Some("inspecting"));
    app.kitty_graphics = true;
    let mut terminal = Terminal::new(TestBackend::new(100, 35)).expect("terminal");
    terminal
        .draw(|f| ui::render(f, &mut app, &ui::Theme::new(false, false), true, "offline"))
        .expect("draw");
    let rect = app.evidence_rect.expect("placement");
    assert!(rect.right() < 100 && rect.bottom() < 35);
    key(&mut app, KeyCode::Char('g'));
    terminal
        .draw(|f| ui::render(f, &mut app, &ui::Theme::new(false, false), true, "offline"))
        .expect("draw");
    assert!(app.evidence_rect.is_none());
    key(&mut app, KeyCode::Char('g'));
    let mut tiny = Terminal::new(TestBackend::new(30, 10)).expect("terminal");
    tiny.draw(|f| ui::render(f, &mut app, &ui::Theme::new(false, false), true, "offline"))
        .expect("draw");
    assert!(app.evidence_rect.is_none());
    key(&mut app, KeyCode::Esc);
    assert!(app.write_ready.is_none());
}
#[test]
fn character_fallback_and_control_sanitizing_never_emit_api_escapes() {
    let mut app = loaded();
    app.evidence.as_mut().expect("panel").documents[0].filename =
        "evil\x1b]52;c;secret\x07.png".into();
    let mut terminal = Terminal::new(TestBackend::new(100, 35)).expect("terminal");
    terminal
        .draw(|f| ui::render(f, &mut app, &ui::Theme::new(false, false), true, "offline"))
        .expect("draw");
    let screen = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(screen.contains('▀'));
    assert!(!screen.contains('\x1b') && !screen.contains('\x07'));
    assert!(screen.contains("Synthetic") || screen.contains("evil"));
    app.color_preview = false;
    terminal
        .draw(|f| ui::render(f, &mut app, &ui::Theme::new(true, false), true, "offline"))
        .expect("draw");
    assert!(!terminal
        .backend()
        .buffer()
        .content
        .iter()
        .any(|c| c.symbol() == "▀"));
}

fn wire(status: u16, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut data = format!(
        "HTTP/1.1 {status} Owned\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    data.extend_from_slice(body);
    data
}
fn fetch_with(
    replies: Vec<Vec<u8>>,
    job: &evidence::Job,
) -> Result<evidence::Content, crate::client::Failure> {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let origin = reqwest::Url::parse(&format!(
        "http://{}",
        listener.local_addr().expect("address")
    ))
    .expect("url");
    let preview_request = job.document.as_ref().map(|d| {
        format!(
            "GET /v1/documents/{}/preview?page={} HTTP/1.1",
            d.id, job.page
        )
    });
    let task = std::thread::spawn(move || {
        for (index, reply) in replies.into_iter().enumerate() {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .expect("timeout");
            let mut request = Vec::new();
            let mut byte = [0; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).expect("request");
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).expect("headers");
            if index == 1 {
                if let Some(expected) = &preview_request {
                    assert!(
                        request.starts_with(expected),
                        "exact requested document and page"
                    );
                }
            }
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer owned-preview-client"));
            let _ = stream.write_all(&reply);
        }
    });
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_static("Bearer owned-preview-client"),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .expect("client");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let result = runtime.block_on(evidence::fetch(&client, &origin, job));
    task.join().expect("fixture");
    result
}
fn access_wire(job: &evidence::Job) -> Vec<u8> {
    let identity = serde_json::json!({"tenant_id":job.owner.tenant,"access":{"subject":job.owner.subject,"capabilities":["workspace.read"]}});
    wire(
        200,
        "Content-Type: application/json\r\n",
        identity.to_string().as_bytes(),
    )
}
#[test]
fn http_preview_checks_identity_hash_size_dimensions_and_redirects() {
    let mut app = loaded();
    // Reload the catalog to obtain a fresh pending job, then queue its image.
    key(&mut app, KeyCode::Char('r'));
    let catalog = app
        .evidence
        .as_mut()
        .expect("panel")
        .take_job()
        .expect("catalog");
    app.finish_evidence(&catalog, evidence::demo(&catalog));
    let job = app
        .evidence
        .as_mut()
        .expect("panel")
        .take_job()
        .expect("image");
    let document = job.document.as_ref().expect("document");
    let pixels = [12u8, 96, 130];
    let sha = hex::encode(ring::digest::digest(&ring::digest::SHA256, &pixels));
    let headers=format!("Content-Type: application/vnd.stateset.rgb8\r\nX-Preview-Width: 1\r\nX-Preview-Height: 1\r\nX-Content-Sha256: {sha}\r\nX-Source-Sha256: {}\r\nX-Document-Id: {}\r\nX-Load-Id: {}\r\n",document.sha256,document.id,job.load_id);
    let result = fetch_with(vec![access_wire(&job), wire(200, &headers, &pixels)], &job)
        .expect("valid pixels");
    match result {
        evidence::Content::Image(image) => assert_eq!(image.rgb, pixels),
        _ => panic!("expected image"),
    }
    for (name, headers, body, status) in [
        (
            "wrong source",
            headers.replace(
                &format!("X-Source-Sha256: {}", document.sha256),
                &format!("X-Source-Sha256: {}", "f".repeat(64)),
            ),
            pixels.to_vec(),
            200,
        ),
        (
            "wrong load",
            headers.replace(
                &format!("X-Load-Id: {}", job.load_id),
                "X-Load-Id: other-load",
            ),
            pixels.to_vec(),
            200,
        ),
        (
            "wrong document",
            headers.replace(
                &format!("X-Document-Id: {}", document.id),
                "X-Document-Id: doc-other",
            ),
            pixels.to_vec(),
            200,
        ),
        (
            "oversized dimensions",
            headers.replace("X-Preview-Width: 1", "X-Preview-Width: 961"),
            pixels.to_vec(),
            200,
        ),
        ("wrong digest", headers.clone(), vec![0; 3], 200),
        ("too many bytes", headers.clone(), vec![0; 4], 200),
        ("too few bytes", headers.clone(), vec![0; 2], 200),
        (
            "wrong media",
            headers.replace("application/vnd.stateset.rgb8", "image/png"),
            pixels.to_vec(),
            200,
        ),
        (
            "redirect",
            "Location: https://refused.invalid/private\r\n".into(),
            vec![],
            302,
        ),
    ] {
        assert!(
            fetch_with(vec![access_wire(&job), wire(status, &headers, &body)], &job).is_err(),
            "{name}"
        );
    }
    // Chunked responses must obey the same exact byte limit, without relying on Content-Length.
    let mut chunked = format!(
        "HTTP/1.1 200 OK\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n{headers}\r\n4\r\n"
    )
    .into_bytes();
    chunked.extend_from_slice(&[1, 2, 3, 4]);
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");
    assert!(fetch_with(vec![access_wire(&job), chunked], &job).is_err());
    let denied = fetch_with(
        vec![access_wire(&job), wire(403, "", b"private server error")],
        &job,
    )
    .err()
    .expect("denied");
    assert!(denied.denied);
    assert!(!denied.message.contains("private server error"));
}
#[test]
fn http_catalog_rejects_cross_load_records_duplicate_ids_and_new_owners() {
    let mut app = app();
    key(&mut app, KeyCode::Char('v'));
    let job = app
        .evidence
        .as_mut()
        .expect("panel")
        .take_job()
        .expect("job");
    let doc = serde_json::json!({"id":"doc-owned","load_id":job.load_id,"filename":"owned.png","kind":"pod","content_type":"image/png","size_bytes":10,"sha256":"a".repeat(64),"status":"pending_review"});
    let mut wrong_load = doc.clone();
    wrong_load["load_id"] = serde_json::json!("other");
    for body in [
        serde_json::json!({"load_id":"other","total":1,"documents":[doc.clone()]}),
        serde_json::json!({"load_id":job.load_id,"total":2,"documents":[doc.clone(),doc.clone()]}),
        serde_json::json!({"load_id":job.load_id,"total":0,"documents":[doc.clone()]}),
        serde_json::json!({"load_id":job.load_id,"total":1,"documents":[wrong_load]}),
    ] {
        assert!(fetch_with(
            vec![
                access_wire(&job),
                wire(
                    200,
                    "Content-Type: application/json\r\n",
                    body.to_string().as_bytes()
                )
            ],
            &job
        )
        .is_err());
    }
    let mut other = job.clone();
    other.owner.subject = "other-operator".into();
    let error = fetch_with(vec![access_wire(&other)], &job)
        .err()
        .expect("changed owner");
    assert!(error.denied);
}

#[test]
fn tmux_commands_wrap_every_chunk_losslessly_and_keep_placements_virtual_and_quiet() {
    use std::io::{Read, Write};
    let mut rgb = vec![0u8; 64 * 64 * 3];
    let mut state = 0x91a78f13u32;
    for byte in &mut rgb {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }
    let mut compressor = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    compressor.write_all(&rgb).expect("compress");
    let pixels = evidence::Pixels {
        page: 1,
        pages: 1,
        width: 64,
        height: 64,
        digest: hex::encode(ring::digest::digest(&ring::digest::SHA256, &rgb)),
        rgb,
        compressed: compressor.finish().expect("finish"),
    };
    let mut renderer = graphics::Renderer::new(0xfe123456, graphics::Transport::Tmux);
    let mut output = Vec::new();
    let area = Rect::new(50, 4, 24, 12);
    renderer
        .update(&mut output, Some((&pixels, area)))
        .expect("wrapped image");
    let text = String::from_utf8(output.clone()).expect("ASCII transport");
    let mut compressed = Vec::new();
    let mut chunks = 0;
    for wrapped in text.split("\x1bPtmux;").skip(1) {
        let command = wrapped
            .strip_suffix("\x1b\\")
            .expect("outer terminator")
            .replace("\x1b\x1b", "\x1b");
        let command = command
            .strip_prefix("\x1b7")
            .expect("save outer cursor")
            .strip_suffix("\x1b8")
            .expect("restore outer cursor");
        let command = command
            .strip_prefix("\x1b_G")
            .expect("APC")
            .strip_suffix("\x1b\\")
            .expect("inner terminator");
        let (header, payload) = command.split_once(';').expect("data separator");
        assert!(header.contains("q=2"));
        if header.contains("a=p") {
            assert!(
                header.contains("U=1")
                    && header.contains("p=1")
                    && header.contains("c=24,r=12,C=1")
            );
        } else {
            assert!(payload.len() <= 4096 && payload.len() % 4 == 0);
            compressed.extend(STANDARD.decode(payload).expect("chunk"));
            chunks += 1;
        }
    }
    assert!(chunks > 1);
    let mut restored = Vec::new();
    flate2::read::ZlibDecoder::new(compressed.as_slice())
        .read_to_end(&mut restored)
        .expect("inflate");
    assert_eq!(restored, pixels.rgb);
    assert!(!text.contains("\x1b[") && !text.contains("a=q"));
    assert_eq!(
        text.matches("\x1b\x1b7").count(),
        text.matches("\x1b\x1b8").count()
    );
    output.clear();
    renderer
        .update(&mut output, Some((&pixels, area)))
        .expect("unchanged");
    assert!(output.is_empty());
    renderer
        .update(&mut output, Some((&pixels, Rect::new(3, 2, 12, 6))))
        .expect("reposition");
    let moved = String::from_utf8(output.clone()).expect("ascii");
    assert!(moved.contains("a=d,d=i,i=4262605910,q=2"));
    assert!(!moved.contains("a=t"));
    output.clear();
    renderer.invalidate(&mut output).expect("clear");
    assert_eq!(
        String::from_utf8(output).expect("ascii"),
        "\x1bPtmux;\x1b\x1b7\x1b\x1b_Ga=d,d=I,i=4262605910,q=2;\x1b\x1b\\\x1b\x1b8\x1b\\"
    );
}

#[test]
fn placeholder_indices_match_the_complete_normative_table_and_full_32bit_identity() {
    let mut bytes = Vec::new();
    for index in 0..=255 {
        let symbol = graphics::placeholder_symbol(0, index, 0).expect("bounded index");
        let row = symbol.chars().nth(1).expect("row");
        bytes.extend((row as u32).to_be_bytes());
        let msb = graphics::placeholder_symbol(u32::from(index) << 24, 0, 0).expect("msb");
        assert_eq!(msb.chars().nth(3), Some(row));
        assert_eq!(
            graphics::placeholder_symbol(0, 0, index)
                .expect("column")
                .chars()
                .nth(2),
            Some(row)
        );
    }
    // Independent SHA of the first 256 primary-source code points as BE u32.
    assert_eq!(
        hex::encode(ring::digest::digest(&ring::digest::SHA256, &bytes)),
        "75d8c736761fb8826fa8d39578449c1a0c1deade3266a3e6145f454b7918543e"
    );
    assert_eq!(
        graphics::placeholder_symbol(0xff123456, 255, 255).expect("last"),
        "\u{10eeee}\u{a8e5}\u{a8e5}\u{a8e5}"
    );
    assert!(graphics::placeholder_symbol(1, 256, 0).is_none());
    assert!(graphics::placeholder_symbol(1, 0, 256).is_none());
}

#[test]
fn tmux_preview_grid_is_owned_pane_relative_bounded_and_removed_by_text_toggle() {
    let mut app = loaded();
    app.kitty_graphics = true;
    app.kitty_image_id = Some(0xfe123456);
    let mut terminal = Terminal::new(TestBackend::new(100, 35)).expect("terminal");
    terminal
        .draw(|f| ui::render(f, &mut app, &ui::Theme::new(false, false), true, "offline"))
        .expect("draw");
    let rect = app.evidence_rect.expect("placement");
    let buffer = terminal.backend().buffer();
    let mut count = 0;
    for y in 0..35 {
        for x in 0..100 {
            let cell = &buffer[(x, y)];
            if cell.symbol().starts_with('\u{10eeee}') {
                assert!(rect.contains((x, y).into()));
                assert_eq!(cell.fg, ratatui::style::Color::Rgb(0x12, 0x34, 0x56));
                assert_eq!(
                    cell.symbol(),
                    graphics::placeholder_symbol(0xfe123456, y - rect.y, x - rect.x)
                        .expect("indices")
                );
                count += 1;
            }
        }
    }
    assert_eq!(count, u32::from(rect.width) * u32::from(rect.height));
    key(&mut app, KeyCode::Char('g'));
    terminal
        .draw(|f| ui::render(f, &mut app, &ui::Theme::new(false, false), true, "offline"))
        .expect("text");
    assert!(app.evidence_rect.is_none());
    assert!(!terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .any(|c| c.symbol().contains('\u{10eeee}')));
}

#[test]
fn tmux_command_boundaries_reject_control_injection_before_emitting_any_bytes() {
    for transport in [graphics::Transport::Direct, graphics::Transport::Tmux] {
        for body in ["a=p;\x1b[2J", "a=p;\n", "a=p;\r", "a=p;\u{9b}"] {
            let mut out = Vec::new();
            assert!(transport.command(&mut out, body).is_err());
            assert!(out.is_empty());
        }
    }
}

#[test]
fn pdf_page_navigation_discards_stale_results_and_resets_at_document_boundaries() {
    let mut app = loaded();
    let panel = app.evidence.as_mut().expect("panel");
    panel.documents[0].content_type = "application/pdf".into();
    panel.pages = Some(3);
    let mut next = panel.documents[0].clone();
    next.id = "second-document".into();
    panel.documents.push(next);
    key(&mut app, KeyCode::PageUp);
    assert_eq!(app.evidence.as_ref().expect("panel").page, 1);
    key(&mut app, KeyCode::PageDown);
    let page2 = app
        .evidence
        .as_mut()
        .expect("panel")
        .take_job()
        .expect("page 2");
    assert_eq!(page2.page, 2);
    assert!(app.evidence.as_ref().expect("panel").pixels.is_none());
    key(&mut app, KeyCode::PageUp);
    let page1 = app
        .evidence
        .as_mut()
        .expect("panel")
        .take_job()
        .expect("page 1");
    let mut stale = evidence::demo(&page2).expect("pixels");
    if let evidence::Content::Image(ref mut pixels) = stale {
        pixels.page = 2;
        pixels.pages = 3;
    }
    app.finish_evidence(&page2, Ok(stale));
    assert!(app.evidence.as_ref().expect("panel").pixels.is_none());
    let mut result = evidence::demo(&page1).expect("pixels");
    if let evidence::Content::Image(ref mut pixels) = result {
        pixels.pages = 3;
    }
    app.finish_evidence(&page1, Ok(result));
    assert!(app.evidence.as_ref().expect("panel").pixels.is_some());
    key(&mut app, KeyCode::PageDown);
    key(&mut app, KeyCode::PageDown);
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.evidence.as_ref().expect("panel").page, 3);
    key(&mut app, KeyCode::Right);
    let panel = app.evidence.as_mut().expect("panel");
    assert_eq!(panel.page, 1);
    assert_eq!(panel.pages, None);
    assert!(panel.pixels.is_none());
    let second = panel.take_job().expect("second document");
    assert_eq!(second.document.expect("document").id, "second-document");
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.evidence.as_ref().expect("panel").page, 1);
    key(&mut app, KeyCode::Char('r'));
    assert!(app.evidence.as_ref().expect("panel").documents.is_empty());
}

#[test]
fn pdf_http_requires_matching_page_and_bounded_total_before_pixels_are_accepted() {
    let mut app = loaded();
    let panel = app.evidence.as_mut().expect("panel");
    panel.documents[0].content_type = "application/pdf".into();
    panel.pages = Some(3);
    key(&mut app, KeyCode::PageDown);
    let job = app
        .evidence
        .as_mut()
        .expect("panel")
        .take_job()
        .expect("page");
    let d = job.document.as_ref().expect("document");
    let pixels = [12u8, 96, 130];
    let sha = hex::encode(ring::digest::digest(&ring::digest::SHA256, &pixels));
    let base = format!("Content-Type: application/vnd.stateset.rgb8\r\nX-Preview-Width: 1\r\nX-Preview-Height: 1\r\nX-Content-Sha256: {sha}\r\nX-Source-Sha256: {}\r\nX-Document-Id: {}\r\nX-Load-Id: {}\r\n", d.sha256, d.id, job.load_id);
    for (pagination, valid) in [
        ("X-Preview-Page: 2\r\nX-Preview-Pages: 3\r\n", true),
        ("", false),
        ("X-Preview-Page: 1\r\nX-Preview-Pages: 3\r\n", false),
        ("X-Preview-Page: 2\r\nX-Preview-Pages: 1\r\n", false),
        ("X-Preview-Page: 2\r\nX-Preview-Pages: 1001\r\n", false),
        ("X-Preview-Page: 0\r\nX-Preview-Pages: 3\r\n", false),
    ] {
        let result = fetch_with(
            vec![
                access_wire(&job),
                wire(200, &format!("{base}{pagination}"), &pixels),
            ],
            &job,
        );
        assert_eq!(result.is_ok(), valid, "{pagination}");
        if let Ok(evidence::Content::Image(p)) = result {
            assert_eq!((p.page, p.pages), (2, 3));
            assert_eq!(p.rgb, pixels);
        }
    }
}

#[test]
fn direct_placement_uses_one_axis_and_preserves_aspect_inside_even_one_cell() {
    use std::io::Write;
    for (w, h) in [(1, 960), (960, 1), (720, 960), (960, 720)] {
        let rgb = [12, 96, 130].repeat(w as usize * h as usize);
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&rgb).expect("compress");
        let pixels = evidence::Pixels {
            width: w,
            height: h,
            digest: "owned".into(),
            rgb,
            compressed: encoder.finish().expect("compressed"),
            page: 1,
            pages: 1,
        };
        for cell in [(8u32, 16u32), (19, 42)] {
            for (columns, rows) in [(1, 1), (11, 7), (40, 25)] {
                let mut renderer = graphics::Renderer::new(710002, graphics::Transport::Direct);
                renderer.set_cell(cell);
                let area = Rect::new(3, 5, columns, rows);
                let mut out = Vec::new();
                renderer
                    .update(&mut out, Some((&pixels, area)))
                    .expect("placement");
                let wire = String::from_utf8(out).expect("ASCII");
                let header = wire
                    .split("\x1b_Ga=p,")
                    .nth(1)
                    .expect("placement")
                    .split(';')
                    .next()
                    .expect("header");
                let fields: std::collections::HashMap<_, _> = header
                    .split(',')
                    .filter_map(|f| f.split_once('='))
                    .collect();
                assert_ne!(
                    fields.contains_key("c"),
                    fields.contains_key("r"),
                    "exactly one dimension"
                );
                let (width, height) = if let Some(c) = fields.get("c") {
                    let width = c.parse::<f64>().expect("columns") * f64::from(cell.0);
                    (width, width * f64::from(h) / f64::from(w))
                } else {
                    let height = fields["r"].parse::<f64>().expect("rows") * f64::from(cell.1);
                    (height * f64::from(w) / f64::from(h), height)
                };
                assert!(width <= f64::from(columns) * f64::from(cell.0) + 0.001);
                assert!(height <= f64::from(rows) * f64::from(cell.1) + 0.001);
                assert!((width / height - f64::from(w) / f64::from(h)).abs() < 0.001);
                let mut changed = Vec::new();
                renderer.set_cell((cell.0 + 1, cell.1 + 1));
                renderer
                    .update(&mut changed, Some((&pixels, area)))
                    .expect("cell metric update");
                let changed = String::from_utf8(changed).expect("ASCII");
                assert!(changed.contains("a=p,"));
                assert!(!changed.contains("a=t,"), "retain unchanged pixel data");
            }
        }
    }
}
#[test]
fn preview_requested_during_first_read_opens_once_rows_arrive() {
    let mut app = App::default();
    assert!(
        app.needs_fetch,
        "a fresh console starts with its first read pending"
    );
    key(&mut app, KeyCode::Char('v'));
    assert!(app.evidence.is_none() && app.evidence_pending);
    app.needs_fetch = false;
    app.apply(demo::snapshot(&app.request()));
    app.tick(std::time::Instant::now());
    assert!(app.evidence.is_some() && !app.evidence_pending);
}
#[test]
fn later_key_cancels_a_preview_waiting_for_its_read() {
    let mut app = App::default();
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Down);
    assert!(!app.evidence_pending);
    app.needs_fetch = false;
    app.apply(demo::snapshot(&app.request()));
    app.tick(std::time::Instant::now());
    assert!(app.evidence.is_none());
}
