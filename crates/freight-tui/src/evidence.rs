//! Read-only evidence viewer. No paths, download URLs or terminal escapes come from the API.
use crate::{
    actions::Owner,
    client::{self, Failure},
    model::{App, View},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use reqwest::{Client, Url};
use serde::Deserialize;
use uuid::Uuid;

pub const SIDE: u32 = 960;
#[derive(Clone, Debug, Deserialize)]
pub struct Document {
    pub id: String,
    pub load_id: String,
    pub filename: String,
    pub kind: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub status: String,
}
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub digest: String,
    pub compressed: Vec<u8>,
    pub page: u32,
    pub pages: u32,
}
impl Pixels {
    fn new(width: u32, height: u32, rgb: Vec<u8>, digest: String) -> Result<Self, Failure> {
        use std::io::Write;
        let mut compressor =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        compressor
            .write_all(&rgb)
            .map_err(|_| fail("Cannot prepare preview"))?;
        let compressed = compressor
            .finish()
            .map_err(|_| fail("Cannot prepare preview"))?;
        Ok(Self {
            width,
            height,
            rgb,
            digest,
            compressed,
            page: 1,
            pages: 1,
        })
    }
}
pub struct Panel {
    pub load_id: String,
    pub reference: String,
    pub owner: Owner,
    pub documents: Vec<Document>,
    pub total: u64,
    pub selected: usize,
    pub pixels: Option<Pixels>,
    pub notice: String,
    pub text_only: bool,
    pub page: u32,
    pub pages: Option<u32>,
    pending: Option<Uuid>,
    ready: bool,
    catalog_loaded: bool,
}
#[derive(Clone)]
pub struct Job {
    pub token: Uuid,
    pub owner: Owner,
    pub load_id: String,
    pub document: Option<Document>,
    pub page: u32,
}
pub enum Content {
    Catalog(Vec<Document>, u64),
    Image(Pixels),
}
fn fail(message: &str) -> Failure {
    Failure {
        message: message.into(),
        denied: false,
    }
}
fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|v| v.is_ascii_hexdigit() && !v.is_ascii_uppercase())
}
impl Panel {
    fn request(&mut self) {
        self.pixels = None;
        self.pending = Some(Uuid::new_v4());
        self.ready = true;
        self.notice = "Loading evidence… Esc returns while the request finishes.".into();
    }
    pub fn document(&self) -> Option<&Document> {
        self.documents.get(self.selected)
    }
    pub fn take_job(&mut self) -> Option<Job> {
        if !self.ready {
            return None;
        }
        self.ready = false;
        Some(Job {
            token: self.pending?,
            owner: self.owner.clone(),
            load_id: self.load_id.clone(),
            document: self
                .catalog_loaded
                .then(|| self.document().cloned())
                .flatten(),
            page: self.page,
        })
    }
    fn finish(&mut self, token: Uuid, result: Result<Content, Failure>) {
        if self.pending != Some(token) {
            return;
        }
        self.pending = None;
        match result {
            Ok(Content::Catalog(documents, total)) => {
                self.documents = documents;
                self.total = total;
                self.selected = 0;
                self.catalog_loaded = true;
                if self.documents.is_empty() {
                    self.notice =
                        "No stored evidence for this shipment. r refreshes the list.".into();
                } else {
                    self.request_image();
                }
            }
            Ok(Content::Image(pixels)) => {
                if pixels.page != self.page {
                    self.notice = "Preview page mismatch refused. r reloads.".into();
                    return;
                }
                self.pages = Some(pixels.pages);
                self.pixels = Some(pixels);
                self.notice =
                    "Preview only · use the web workspace to review or download the original."
                        .into();
            }
            Err(error) => self.notice = error.message,
        }
    }
    fn request_image(&mut self) {
        self.pixels = None;
        self.pending = None;
        self.ready = false;
        if self.document().is_some_and(|d| {
            matches!(
                d.content_type.as_str(),
                "image/png" | "image/jpeg" | "application/pdf"
            )
        }) {
            self.request();
        } else {
            self.notice =
                "Preview unavailable for this format. Open the original in the web workspace."
                    .into();
        }
    }
}
impl App {
    pub fn open_evidence(&mut self) {
        if self.view == View::Loads
            && (self.loading.is_some() || self.needs_fetch)
            && !self.denied
            && !self.quit_confirm
        {
            // The rows may still be arriving (for example the first read);
            // keep the request instead of silently dropping it.
            self.evidence_pending = true;
            return;
        }
        if self.view != View::Loads
            || self.loading.is_some()
            || self.needs_fetch
            || self.denied
            || self.write_busy
            || self.write_ready.is_some()
            || self.quit_confirm
        {
            return;
        }
        let Some(owner) = Owner::from_identity(&self.identity) else {
            return;
        };
        let Some(record) = self.current().filter(|r| valid_id(&r.id)) else {
            return;
        };
        let mut panel = Panel {
            load_id: record.id.clone(),
            reference: record.cells.first().cloned().unwrap_or_default(),
            owner,
            documents: vec![],
            total: 0,
            selected: 0,
            pixels: None,
            notice: String::new(),
            text_only: false,
            page: 1,
            pages: None,
            pending: None,
            ready: false,
            catalog_loaded: false,
        };
        panel.request();
        self.evidence = Some(panel);
    }
    pub fn evidence_key(&mut self, key: KeyEvent) -> bool {
        let Some(panel) = self.evidence.as_mut() else {
            return false;
        };
        if !key.modifiers.is_empty() {
            return true;
        }
        if key.kind == KeyEventKind::Repeat
            && !matches!(
                key.code,
                KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Char('j' | 'k' | 'h' | 'l')
            )
        {
            return true;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('v') => self.evidence = None,
            KeyCode::Char('q') => {
                self.evidence = None;
                self.request_quit();
            }
            KeyCode::Char('g') => panel.text_only = !panel.text_only,
            KeyCode::Char('r') => {
                panel.catalog_loaded = false;
                panel.documents.clear();
                panel.page = 1;
                panel.pages = None;
                panel.request();
            }
            KeyCode::PageDown if panel.pages.is_some_and(|pages| panel.page < pages) => {
                panel.page += 1;
                panel.request_image();
            }
            KeyCode::PageUp if panel.page > 1 => {
                panel.page -= 1;
                panel.request_image();
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j' | 'l') => {
                if panel.selected + 1 < panel.documents.len() {
                    panel.selected += 1;
                    panel.page = 1;
                    panel.pages = None;
                    panel.request_image();
                }
            }
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k' | 'h') => {
                if panel.selected > 0 {
                    panel.selected -= 1;
                    panel.page = 1;
                    panel.pages = None;
                    panel.request_image();
                }
            }
            _ => {}
        }
        true
    }
    pub fn finish_evidence(&mut self, job: &Job, result: Result<Content, Failure>) {
        // Even an obsolete denial clears previously displayed private evidence.
        if result.as_ref().is_err_and(|error| error.denied) {
            let message = result.err().map(|e| e.message).unwrap_or_default();
            self.clear_access();
            self.error = Some(message);
            self.needs_fetch = false;
            return;
        }
        if Owner::from_identity(&self.identity).as_ref() != Some(&job.owner) {
            return;
        }
        if let Some(panel) = self
            .evidence
            .as_mut()
            .filter(|p| p.owner == job.owner && p.load_id == job.load_id)
        {
            panel.finish(job.token, result);
        }
    }
}
async fn response(client: &Client, url: Url) -> Result<reqwest::Response, Failure> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| fail("Evidence request interrupted. r retries; Esc returns."))?;
    match response.status().as_u16() {
        200 => Ok(response),
        401 | 403 => Err(Failure { message:"Access denied. Records and evidence cleared.".into(), denied:true }),
        404 => Err(fail("Evidence unavailable on this server. r retries; use the web workspace.")),
        422 => Err(fail("This document or page cannot be previewed within the safety limits. Use the original in the web workspace.")),
        300..=399 => Err(fail("Evidence redirect refused. Check the API origin.")),
        _ => Err(fail("Evidence unavailable or busy. r retries; Esc returns.")),
    }
}
pub async fn fetch(client: &Client, origin: &Url, job: &Job) -> Result<Content, Failure> {
    let identity = client::access(client, origin).await?;
    if Owner::from_identity(&identity).as_ref() != Some(&job.owner) {
        return Err(Failure {
            message: "Identity changed. Records and evidence cleared; r reloads.".into(),
            denied: true,
        });
    }
    if !valid_id(&job.load_id) {
        return Err(fail("Invalid shipment identifier"));
    }
    let mut url = origin.clone();
    if let Some(document) = &job.document {
        if !valid_id(&document.id)
            || document.load_id != job.load_id
            || !digest(&document.sha256)
            || job.page == 0
            || job.page > 1000
        {
            return Err(fail("Invalid document identity"));
        }
        url.set_path(&format!("/v1/documents/{}/preview", document.id));
        url.set_query(Some(&format!("page={}", job.page)));
        let mut response = response(client, url).await?;
        let headers = response.headers();
        let field = |name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
        };
        let w = field("x-preview-width").parse::<u32>().unwrap_or(0);
        let h = field("x-preview-height").parse::<u32>().unwrap_or(0);
        // Old image-only servers remain compatible. PDF responses must bind explicit pagination.
        let pdf = document.content_type == "application/pdf";
        let page = if !pdf && field("x-preview-page").is_empty() {
            1
        } else {
            field("x-preview-page").parse::<u32>().unwrap_or(0)
        };
        let pages = if !pdf && field("x-preview-pages").is_empty() {
            1
        } else {
            field("x-preview-pages").parse::<u32>().unwrap_or(0)
        };
        let expected = field("x-content-sha256").to_owned();
        if w == 0
            || h == 0
            || w > SIDE
            || h > SIDE
            || page != job.page
            || page == 0
            || pages < page
            || pages > 1000
            || !pdf && (page != 1 || pages != 1)
            || !digest(&expected)
            || field("content-type") != "application/vnd.stateset.rgb8"
            || field("x-source-sha256") != document.sha256
            || field("x-document-id") != document.id
            || field("x-load-id") != job.load_id
        {
            return Err(fail("Preview identity, integrity or dimensions refused"));
        }
        let length = w as usize * h as usize * 3;
        if response
            .content_length()
            .is_some_and(|n| n != length as u64)
        {
            return Err(fail("Preview byte count refused"));
        }
        let mut rgb = Vec::with_capacity(length);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| fail("Preview interrupted. r retries."))?
        {
            if chunk.len() > length.saturating_sub(rgb.len()) {
                return Err(fail("Oversized preview refused"));
            }
            rgb.extend_from_slice(&chunk);
        }
        let actual = hex::encode(ring::digest::digest(&ring::digest::SHA256, &rgb));
        if rgb.len() != length || actual != expected {
            return Err(fail("Preview integrity check failed"));
        }
        let mut pixels = Pixels::new(w, h, rgb, expected)?;
        pixels.page = page;
        pixels.pages = pages;
        Ok(Content::Image(pixels))
    } else {
        url.set_path(&format!("/v1/loads/{}/documents", job.load_id));
        let data = client::read_json(response(client, url).await?, 256 * 1024).await?;
        let documents: Vec<Document> = serde_json::from_value(data["documents"].clone())
            .map_err(|_| fail("Invalid evidence catalog"))?;
        let total = data["total"]
            .as_u64()
            .ok_or_else(|| fail("Missing evidence count"))?;
        let mut ids = std::collections::HashSet::new();
        if data["load_id"] != job.load_id
            || documents.len() > 100
            || total < documents.len() as u64
            || documents.iter().any(|d| {
                !valid_id(&d.id)
                    || d.load_id != job.load_id
                    || !digest(&d.sha256)
                    || d.filename.len() > 180
                    || !ids.insert(&d.id)
            })
        {
            return Err(fail("Evidence catalog identity or bounds refused"));
        }
        Ok(Content::Catalog(documents, total))
    }
}

pub fn demo(job: &Job) -> Result<Content, Failure> {
    if job.document.is_none() {
        return Ok(Content::Catalog(
            vec![Document {
                id: "doc-demo-pod".into(),
                load_id: job.load_id.clone(),
                filename: "Synthetic POD.png".into(),
                kind: "pod".into(),
                content_type: "image/png".into(),
                size_bytes: 0,
                sha256: "0".repeat(64),
                status: "pending_review".into(),
            }],
            1,
        ));
    }
    // A clearly synthetic paper specimen, never represented as real evidence.
    let (width, height) = (360, 480);
    let mut rgb = vec![250; width as usize * height as usize * 3];
    for y in 0..height {
        for x in 0..width {
            let color = if (24..336).contains(&x) && (24..86).contains(&y) {
                [20, 72, 89]
            } else if (30..330).contains(&x) && (120..360).contains(&y) && y % 36 < 2 {
                [170, 180, 185]
            } else if (30..180).contains(&x) && (395..430).contains(&y) {
                [80, 150, 160]
            } else {
                [250, 250, 247]
            };
            let n = (y as usize * width as usize + x as usize) * 3;
            rgb[n..n + 3].copy_from_slice(&color);
        }
    }
    let digest = hex::encode(ring::digest::digest(&ring::digest::SHA256, &rgb));
    Ok(Content::Image(Pixels::new(width, height, rgb, digest)?))
}
