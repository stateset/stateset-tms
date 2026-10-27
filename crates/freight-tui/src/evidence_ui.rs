use crate::{evidence::Panel, graphics, ui::Theme};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
    Frame,
};

pub fn render(
    frame: &mut Frame,
    panel: &Panel,
    theme: &Theme,
    kitty: bool,
    placeholder: Option<u32>,
    color: bool,
    cell: (u32, u32),
) -> Option<Rect> {
    let area = frame.area();
    if area.width < 52 || area.height < 16 {
        return None;
    }
    let area = Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2);
    frame.render_widget(Clear, area);
    let block = theme.block(format!(
        " Evidence · {} ",
        crate::model::clean(&panel.reference)
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let title = if let Some(d) = panel.document() {
        format!(
            "{} / {} shown · {} total · {}\n{} · {} · {} bytes · {}\nOriginal SHA256 {} · Page {} / {}",
            panel.selected + 1,
            panel.documents.len(),
            panel.total,
            crate::model::clean(&d.filename),
            crate::model::clean(&d.kind),
            crate::model::clean(&d.status),
            d.size_bytes,
            crate::model::clean(&d.content_type),
            d.sha256,
            panel.page,
            panel.pages.map(|pages| pages.to_string()).unwrap_or_else(|| "?".into())
        )
    } else {
        "Shipment evidence · read only".into()
    };
    let metadata_only = panel.text_only || !color && !kitty;
    let metadata = Paragraph::new(title).wrap(Wrap { trim: false });
    let header_height = if metadata_only {
        0
    } else {
        metadata
            .line_count(inner.width)
            .min(usize::from(inner.height.saturating_sub(6))) as u16
    };
    let regions = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(inner);
    frame.render_widget(
        metadata,
        if metadata_only {
            regions[1]
        } else {
            regions[0]
        },
    );
    let mut placement = None;
    if let Some(pixels) = &panel.pixels {
        if !metadata_only {
            let mut bounds = regions[1];
            if placeholder.is_some() {
                let width = bounds.width.min(256);
                let height = bounds.height.min(256);
                bounds = Rect::new(
                    bounds.x + (bounds.width - width) / 2,
                    bounds.y + (bounds.height - height) / 2,
                    width,
                    height,
                );
            }
            let rect = graphics::fit(pixels, bounds, cell);
            if kitty {
                placement = Some(rect);
                if let Some(id) = placeholder {
                    graphics::placeholders(frame, id, rect);
                }
            } else if color {
                let lines = (0..rect.height)
                    .map(|row| {
                        Line::from(
                            (0..rect.width)
                                .map(|col| {
                                    let sample = |sub: u32| {
                                        let x =
                                            u32::from(col) * pixels.width / u32::from(rect.width);
                                        let y = (u32::from(row) * 2 + sub) * pixels.height
                                            / (u32::from(rect.height) * 2);
                                        let n =
                                            (y as usize * pixels.width as usize + x as usize) * 3;
                                        Color::Rgb(
                                            pixels.rgb[n],
                                            pixels.rgb[n + 1],
                                            pixels.rgb[n + 2],
                                        )
                                    };
                                    Span::styled("▀", Style::default().fg(sample(0)).bg(sample(1)))
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>();
                frame.render_widget(Paragraph::new(lines), rect);
            }
        }
    }
    let mode = if panel.text_only || !color && !kitty {
        "Text metadata"
    } else if kitty && placeholder.is_some() {
        "Kitty graphics · tmux"
    } else if kitty {
        "Kitty graphics"
    } else {
        "Character preview · --graphics kitty for a compatible terminal"
    };
    frame.render_widget(
        Paragraph::new(format!("{mode} · {}", crate::model::clean(&panel.notice)))
            .wrap(Wrap { trim: false }),
        regions[2],
    );
    frame.render_widget(
        Paragraph::new("←/→ docs · PgUp/Dn pages · g text · r · Esc back")
            .style(Style::default().fg(theme.muted)),
        regions[3],
    );
    placement
}
