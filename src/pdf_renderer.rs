//! PDF process boundary and authenticated, bounded local renderer transport.
use std::path::PathBuf;

#[path = "pdf_renderer/local.rs"]
mod local;
#[cfg(target_os = "linux")]
#[path = "pdf_renderer/service.rs"]
mod service;

pub const MAX_PAGES: u32 = 1000;
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Invalid,
    Unavailable,
    Busy,
}

#[derive(Debug)]
pub(crate) struct Rendered {
    pub png: Vec<u8>,
    pub pages: u32,
}

#[derive(Default)]
pub struct PdfRenderer {
    #[cfg(target_os = "linux")]
    service: Option<ServiceConfig>,
}

#[cfg(target_os = "linux")]
struct ServiceConfig {
    path: PathBuf,
    uid: u32,
}

impl PdfRenderer {
    pub fn from_env() -> Result<Self, &'static str> {
        let path = std::env::var_os("FREIGHT_PDF_RENDERER_SOCKET");
        let uid = std::env::var("FREIGHT_PDF_RENDERER_UID");
        match (path, uid) {
            (None, Err(std::env::VarError::NotPresent)) => Ok(Self::default()),
            (Some(path), uid) => Self::service(
                PathBuf::from(path),
                match uid {
                    Ok(value) => value.parse().map_err(|_| "Invalid PDF renderer UID")?,
                    Err(std::env::VarError::NotPresent) => 10002,
                    Err(_) => return Err("Invalid PDF renderer UID"),
                },
            ),
            _ => Err("PDF renderer UID requires a socket"),
        }
    }

    pub fn service(path: PathBuf, uid: u32) -> Result<Self, &'static str> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (path, uid);
            return Err("PDF renderer service requires Linux");
        }
        #[cfg(target_os = "linux")]
        {
            let name = path.to_str().ok_or("Invalid PDF renderer socket")?;
            if !path.is_absolute()
                || name.is_empty()
                || name.len() > 100
                || name.as_bytes().contains(&0)
                || path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
                || uid == 0
            {
                return Err("Invalid PDF renderer socket or UID");
            }
            Ok(Self {
                service: Some(ServiceConfig { path, uid }),
            })
        }
    }

    pub(crate) async fn render(&self, bytes: &[u8], page: u32) -> Result<Rendered, Failure> {
        if bytes.is_empty()
            || bytes.len() > crate::documents::MAX_DOCUMENT_BYTES
            || page == 0
            || page > MAX_PAGES
        {
            return Err(Failure::Invalid);
        }
        #[cfg(target_os = "linux")]
        if let Some(config) = &self.service {
            return service::request(config, bytes, page).await;
        }
        local::render(bytes, page, false).await
    }
}

/// Internal container entry. It never initializes API credentials or storage.
#[doc(hidden)]
pub fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    {
        service::entry()
    }
    #[cfg(not(target_os = "linux"))]
    Err("PDF renderer service requires Linux".into())
}

#[doc(hidden)]
pub fn check_service() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    {
        service::check()
    }
    #[cfg(not(target_os = "linux"))]
    Err("PDF renderer service requires Linux".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn socket_configuration_refuses_relative_overlong_nul_and_privileged_peers() {
        for (path, uid) in [
            ("relative", 10002),
            ("/a/../b", 10002),
            ("/socket", 0),
            ("/socket\0", 10002),
        ] {
            assert!(PdfRenderer::service(path.into(), uid).is_err());
        }
        assert!(PdfRenderer::service(("/".to_string() + &"x".repeat(101)).into(), 10002).is_err());
        #[cfg(target_os = "linux")]
        assert!(PdfRenderer::service("/run/freight-pdf/renderer.sock".into(), 10002).is_ok());
    }
}
