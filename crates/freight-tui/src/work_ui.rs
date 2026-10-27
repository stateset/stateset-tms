use crate::{
    actions::{Dialog, Kind, Stage},
    model::{clean, App},
    ui::Theme,
};
use ratatui::{
    layout::Rect,
    style::Style,
    widgets::{Clear, Paragraph, Wrap},
    Frame,
};

pub fn action(frame: &mut Frame, app: &App, dialog: &Dialog, theme: &Theme, area: Rect) -> bool {
    let width = area.width.min(94);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + 1,
        width,
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, rect);
    if !app.action_viewport_ready {
        frame.render_widget(
            Paragraph::new("Resize to at least 72 columns × 22 rows to review work. Esc closes.")
                .wrap(Wrap { trim: false }),
            rect,
        );
        return false;
    }
    let Some(work) = dialog.target.work.as_ref() else {
        return false;
    };
    let mut lines = vec![
        format!("{} · {}", dialog.target.reference, dialog.target.id),
        format!(
            "Work version {} · {}",
            dialog.target.version, dialog.target.status
        ),
        format!(
            "Operator: {} · {}",
            clean(&dialog.owner.tenant),
            clean(&dialog.owner.subject)
        ),
        format!(
            "Current owner: {}",
            work.owner.as_deref().unwrap_or("Unowned")
        ),
        format!(
            "Source: {} · priority P{}",
            if work.source_active {
                "active"
            } else {
                "inactive"
            },
            work.priority
        ),
        format!("Source hash: {}", work.source_sha256),
        String::new(),
    ];
    match dialog.stage {
        Stage::Choose => {
            for (i, kind) in Kind::choices(&dialog.target).iter().enumerate() {
                lines.push(format!(
                    "{}  {}{}",
                    i + 1,
                    kind.title(),
                    if kind.allowed_for(&dialog.target, &dialog.owner) {
                        ""
                    } else {
                        " (unavailable)"
                    }
                ));
            }
            lines.push("Choose 1–6. Esc closes without saving.".into());
        }
        Stage::Time => {
            lines.push(dialog.kind.title().into());
            lines.push(format!(
                "Current UTC: {}",
                chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
            ));
            lines.push(format!(
                "Existing follow-up: {}",
                work.follow_up_at.as_deref().unwrap_or("None")
            ));
            lines.push("Time: 30m, 2h, 1d, or RFC3339 with Z/offset".into());
            lines.push(format!("{}▏", dialog.when));
            lines.push("Enter pins the absolute UTC time · Esc discards".into());
        }
        Stage::Edit | Stage::Review => {
            lines.push(dialog.kind.title().into());
            if !dialog.when.is_empty() {
                lines.push(format!("Reviewed UTC time: {}", dialog.when));
            }
            lines.push("Findings / coordination (1–200 UTF-8 bytes):".into());
            lines.push(format!(
                "{}{}",
                dialog.input,
                if dialog.stage == Stage::Edit {
                    "▏"
                } else {
                    ""
                }
            ));
            lines.push(
                if dialog.stage == Stage::Edit {
                    "Enter reviews · Esc discards"
                } else {
                    "y confirms and saves · Esc discards"
                }
                .into(),
            );
        }
        Stage::StopEdit | Stage::StopReview => {
            lines.push("Resolve the uncertain original work action".into());
            lines.push("Retain its completed result, or prevent later application.".into());
            lines.push(format!(
                "Request: {}",
                app.pending.as_ref().map(|p| p.key.as_str()).unwrap_or("—")
            ));
            lines.push(format!(
                "Resolution note: {}{}",
                dialog.input,
                if dialog.stage == Stage::StopEdit {
                    "▏"
                } else {
                    ""
                }
            ));
            lines.push(
                if dialog.stage == Stage::StopEdit {
                    "Enter reviews · Esc returns to recovery"
                } else {
                    "y resolves original · Esc returns to recovery"
                }
                .into(),
            );
        }
    }
    lines.push(String::new());
    lines.push("Work actions keep shipment source conditions unchanged.".into());
    if let Some(error) = &dialog.error {
        lines.push(error.clone());
    }
    let widget = Paragraph::new(lines.join("\n"))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .block(theme.block(" Reviewed work action "));
    let fits = widget.line_count(rect.width) <= usize::from(rect.height);
    if fits {
        frame.render_widget(widget, rect)
    } else {
        frame.render_widget(
            Paragraph::new(
                "This review does not fit. Enlarge the window before confirming. Esc closes.",
            )
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.warning).bg(theme.background)),
            rect,
        )
    }
    fits
}
