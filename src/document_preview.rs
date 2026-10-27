//! Authenticated, bounded image derivatives. Originals and review state stay unchanged.
use super::*;
use image::{ImageDecoder, ImageReader};
use serde::Deserialize;
use std::io::Cursor;
use tokio::sync::Semaphore;

use crate::pdf_renderer;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreviewQuery {
    page: Option<u32>,
}

// The detached worker owns the permit through decode or child termination/reaping,
// even if the HTTP request is cancelled. There is no unbounded preview queue.
static DECODERS: Semaphore = Semaphore::const_new(1);
const SIDE: u32 = 960;
const MAX_PIXELS: u64 = 16_000_000;

fn pdf_error(error: pdf_renderer::Failure) -> Error {
    match error {
        pdf_renderer::Failure::Invalid => Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "PDF is invalid, protected or exceeds preview limits; use the original".into(),
        ),
        pdf_renderer::Failure::Unavailable => Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "PDF renderer unavailable; use the original in the web workspace".into(),
        ),
        pdf_renderer::Failure::Busy => Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Document preview is busy".into(),
        ),
    }
}

pub(super) async fn list(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let workspace = app.store.read(&tenant.0).await?;
    if !workspace.loads.iter().any(|load| load.id == id) {
        return Err("Load not found".to_owned().into());
    }
    let mut documents: Vec<_> = workspace
        .documents
        .iter()
        .filter(|d| d.load_id == id)
        .collect();
    documents.sort_by(|a, b| b.uploaded_at.cmp(&a.uploaded_at).then(a.id.cmp(&b.id)));
    let total = documents.len();
    // No review notes, document bytes or unrelated shipment data in this catalog.
    let documents: Vec<_> = documents
        .into_iter()
        .take(100)
        .map(|d| {
            json!({
                "id":d.id,"load_id":d.load_id,"filename":d.filename,"kind":d.kind,
                "content_type":d.content_type,"size_bytes":d.size_bytes,"sha256":d.sha256,
                "status":d.status(),"version":d.version
            })
        })
        .collect();
    Ok((
        [("cache-control", "private, no-store")],
        Json(json!({
            "load_id":id,"documents":documents,"total":total
        })),
    )
        .into_response())
}

pub(super) async fn preview(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    Query(query): Query<PreviewQuery>,
) -> Result<Response, Error> {
    let page = query.page.unwrap_or(1);
    if page == 0 || page > pdf_renderer::MAX_PAGES {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid preview page".into(),
        ));
    }
    let permit = DECODERS.try_acquire().map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Document preview is busy".into(),
        )
    })?;
    let (document, bytes) = app.store.document_content(&tenant.0, &id).await?;
    let media = document.content_type.clone();
    let renderer = app.pdf_renderer.clone();
    let (width, height, pixels, pages) = tokio::spawn(async move {
        let _permit = permit;
        if media == "application/pdf" {
            let rendered = renderer.render(&bytes, page).await.map_err(pdf_error)?;
            let (w, h, rgb) =
                tokio::task::spawn_blocking(move || raster("image/png", &rendered.png))
                    .await
                    .map_err(|_| {
                        Error(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "Invalid PDF preview".into(),
                        )
                    })?
                    .map_err(|_| {
                        Error(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "Invalid PDF preview".into(),
                        )
                    })?;
            Ok((w, h, rgb, rendered.pages))
        } else if page != 1 {
            Err(Error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Image has one page".into(),
            ))
        } else {
            tokio::task::spawn_blocking(move || raster(&media, &bytes))
                .await
                .map_err(|_| {
                    Error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Document preview failed".into(),
                    )
                })?
                .map(|(w, h, pixels)| (w, h, pixels, 1))
                .map_err(|message| Error(StatusCode::UNPROCESSABLE_ENTITY, message.into()))
        }
    })
    .await
    .map_err(|_| {
        Error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Document preview failed".into(),
        )
    })??;
    let digest = crate::documents::sha256(&pixels);
    Ok((
        [
            ("content-type", "application/vnd.stateset.rgb8".to_owned()),
            ("cache-control", "private, no-store".to_owned()),
            ("x-content-type-options", "nosniff".to_owned()),
            ("x-preview-width", width.to_string()),
            ("x-preview-height", height.to_string()),
            ("x-preview-page", page.to_string()),
            ("x-preview-pages", pages.to_string()),
            ("x-content-sha256", digest),
            ("x-source-sha256", document.sha256),
            ("x-document-id", document.id),
            ("x-load-id", document.load_id),
        ],
        pixels,
    )
        .into_response())
}

fn raster(media: &str, bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), &'static str> {
    let format = match media {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" => image::ImageFormat::Jpeg,
        _ => return Err("Preview supports PNG, JPEG and PDF"),
    };
    if bytes.is_empty() || bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("Document exceeds the preview limit");
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "Invalid or oversized document image")?;
    let (w, h) = decoder.dimensions();
    if w == 0
        || h == 0
        || u64::from(w) * u64::from(h) > MAX_PIXELS
        || decoder.total_bytes() > 64 * 1024 * 1024
    {
        return Err("Image exceeds the 16 megapixel preview limit");
    }
    // Dimensions are checked before allocating decoded pixels. Only PNG/JPEG
    // decoders are compiled in; no delegates, subprocesses, URLs or file paths.
    let orientation = decoder
        .orientation()
        .map_err(|_| "Invalid image orientation")?;
    let mut image = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| "Invalid or oversized document image")?;
    image.apply_orientation(orientation);
    let resized = if image.width() > SIDE || image.height() > SIDE {
        let resized = image.thumbnail(SIDE, SIDE);
        drop(image);
        resized
    } else {
        image
    };
    let rgba = resized.into_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for p in rgba.pixels() {
        for channel in &p.0[..3] {
            // Composite transparent paper onto white; do not turn it black.
            rgb.push(
                ((u16::from(*channel) * u16::from(p[3]) + 255 * (255 - u16::from(p[3])) + 127)
                    / 255) as u8,
            );
        }
    }
    Ok((width, height, rgb))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalized_pixels_are_bounded_and_transparent_paper_is_white() {
        let image = image::RgbaImage::from_pixel(1200, 600, image::Rgba([255, 0, 0, 0]));
        let mut png = Cursor::new(Vec::new());
        image
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png");
        let (w, h, rgb) = raster("image/png", png.get_ref()).expect("preview");
        assert_eq!((w, h), (960, 480));
        assert_eq!(rgb.len(), 960 * 480 * 3);
        assert!(rgb.iter().all(|v| *v == 255));
        assert!(raster("image/jpeg", png.get_ref()).is_err());
        assert!(raster("image/png", b"\x89PNG\r\n\x1a\n").is_err());
        assert!(raster("application/pdf", b"%PDF-1.4").is_err());
    }
    #[test]
    fn oversized_dimensions_are_refused_before_pixel_decode() {
        let image = image::GrayImage::new(8193, 1);
        let mut png = Cursor::new(Vec::new());
        image
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png");
        assert!(raster("image/png", png.get_ref()).is_err());
        let image = image::GrayImage::new(4096, 4096);
        let mut png = Cursor::new(Vec::new());
        image
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png");
        assert!(raster("image/png", png.get_ref()).is_err());
    }

    #[test]
    fn small_jpeg_is_not_enlarged() {
        let image = image::RgbImage::from_pixel(32, 16, image::Rgb([12, 96, 130]));
        let mut jpeg = Cursor::new(Vec::new());
        image
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .expect("jpeg");
        let (w, h, rgb) = raster("image/jpeg", jpeg.get_ref()).expect("preview");
        assert_eq!((w, h, rgb.len()), (32, 16, 32 * 16 * 3));
        assert!((i16::from(rgb[0]) - 12).abs() < 5);
        assert!(raster("image/png", jpeg.get_ref()).is_err());
    }
}
