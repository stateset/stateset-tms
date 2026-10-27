use crate::model::{clean, money, text, App, View, STATUSES};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, Tabs, Wrap},
    Frame,
};
use unicode_segmentation::UnicodeSegmentation;

pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub accent: Color,
    pub warning: Color,
    pub selection: Color,
}
impl Theme {
    pub fn new(no_color: bool, light: bool) -> Self {
        if no_color {
            return Self {
                background: Color::Reset,
                foreground: Color::Reset,
                muted: Color::Reset,
                accent: Color::Reset,
                warning: Color::Reset,
                selection: Color::Reset,
            };
        }
        if light {
            return Self {
                background: Color::Rgb(247, 249, 252),
                foreground: Color::Rgb(25, 36, 54),
                muted: Color::Rgb(90, 105, 125),
                accent: Color::Rgb(0, 105, 130),
                warning: Color::Rgb(151, 71, 0),
                selection: Color::Rgb(220, 237, 245),
            };
        }
        Self {
            background: Color::Rgb(13, 20, 32),
            foreground: Color::Rgb(222, 231, 242),
            muted: Color::Rgb(135, 154, 177),
            accent: Color::Rgb(92, 215, 216),
            warning: Color::Rgb(255, 189, 105),
            selection: Color::Rgb(28, 50, 68),
        }
    }
    pub(crate) fn block(&self, title: impl Into<Line<'static>>) -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.muted))
            .title(title)
    }
    fn status(&self, value: &str) -> Style {
        Style::default().fg(
            if ["attention", "retry", "held", "failed", "credit_due"].contains(&value) {
                self.warning
            } else if ["in_transit", "delivered", "reconciled", "sent", "applied"].contains(&value)
            {
                self.accent
            } else {
                self.foreground
            },
        )
    }
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
fn input_editor(draft: &str, cursor: usize, width: u16) -> (Line<'static>, u16) {
    let before = &draft[..cursor];
    let mut start = cursor;
    let mut used = 0;
    let budget = usize::from(width.saturating_sub(6));
    for (index, grapheme) in before.grapheme_indices(true).rev() {
        let cells = Span::raw(grapheme).width();
        if used + cells > budget {
            break;
        }
        start = index;
        used += cells;
    }
    let clipped_left = start > 0;
    let column = 3 + usize::from(clipped_left) + used;
    let room = usize::from(width).saturating_sub(column + 2);
    let mut end = cursor;
    let mut used = 0;
    for grapheme in draft[cursor..].graphemes(true) {
        let cells = Span::raw(grapheme).width();
        if used + cells > room {
            break;
        }
        end += grapheme.len();
        used += cells;
    }
    (
        Line::from(vec![
            Span::raw(" / "),
            Span::raw(if clipped_left { "‹" } else { "" }),
            Span::raw(draft[start..cursor].to_owned()),
            Span::styled("▏", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(draft[cursor..end].to_owned()),
            Span::raw(if end < draft.len() { "›" } else { "" }),
        ]),
        u16::try_from(column).unwrap_or(width.saturating_sub(1)),
    )
}
pub fn render(frame: &mut Frame, app: &mut App, theme: &Theme, demo: bool, origin: &str) {
    let area = frame.area();
    app.evidence_rect = None;
    app.action_viewport_ready = area.width >= 72 && area.height >= 22;
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background).fg(theme.foreground)),
        area,
    );
    if area.width < 52 || area.height < 16 {
        if app.palette.is_some() {
            frame.render_widget(Paragraph::new("StateSet Freight\n\nResize to at least 52 columns × 16 rows.\nEsc closes commands; Ctrl+C exits.").wrap(Wrap { trim: false }), area);
            return;
        }
        frame.render_widget(Paragraph::new("StateSet Freight\n\nResize to at least 52 columns × 16 rows.\nPress q or Ctrl+C to exit.").wrap(Wrap { trim: false }),area);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(if area.height < 22 { 0 } else { 3 }),
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);
    let badge = if demo {
        "DEMO · SYNTHETIC DATA"
    } else if crate::actions::capability(&app.identity, "operations.write") {
        "API · OPERATIONS"
    } else {
        "API · READ ONLY"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    " STATESET ",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "FREIGHT  /  TERMINAL",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("    {}{badge}", if app.stale() { "STALE · " } else { "" }),
                    Style::default().fg(theme.warning),
                ),
            ]),
            Line::from(Span::styled(
                format!(
                    " {} · {} · {}",
                    text(&app.identity["tenant_id"]),
                    text(&app.identity["access"]["role"]),
                    if demo { "Offline exploration" } else { origin }
                ),
                Style::default().fg(theme.muted),
            )),
        ]),
        chunks[0],
    );
    let titles = View::ALL
        .iter()
        .enumerate()
        .map(|(i, v)| {
            format!(
                "{} {}",
                i + 1,
                if area.width < 85 && *v == View::Integrations {
                    "Queues"
                } else if area.width < 85 && *v == View::Operations {
                    "Ops"
                } else {
                    v.title()
                }
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Tabs::new(titles)
            .block(theme.block(" Workspaces · Ctrl+P commands "))
            .select(app.view.index())
            .highlight_style(
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .divider("│"),
        chunks[1],
    );
    let stats = if matches!(app.view, View::Billing | View::Payables) {
        // Labels keep "USD" for USD-only tenants; mixed tenants list each
        // currency separately because amounts are never converted or summed.
        [
            ("All receivable", "Receivable", "receivable_cents"),
            ("All overdue", "Overdue", "overdue_cents"),
            ("All payable", "Payable", "prepared_payable_cents"),
        ]
        .into_iter()
        .map(|(all, page, field)| {
            let name = if app.catalog() { all } else { page };
            match crate::model::currency_totals(&app.finance, field) {
                Some(totals) => (name.to_string(), totals),
                None => (format!("{name} USD"), money(&app.finance[field])),
            }
        })
        .collect::<Vec<_>>()
    } else {
        let attention = app.rows.iter().filter(|r| r.needs_attention()).count();
        vec![
            (
                if app.paged() {
                    "Rows on page"
                } else {
                    "Records"
                },
                app.rows.len().to_string(),
            ),
            ("Needs attention", attention.to_string()),
            (
                "Snapshot age",
                app.updated
                    .map(|v| {
                        format!(
                            "{}{}s",
                            if app.stale() { "STALE " } else { "" },
                            v.elapsed().as_secs()
                        )
                    })
                    .unwrap_or_else(|| "—".into()),
            ),
        ]
        .into_iter()
        .map(|(title, value)| (title.to_string(), value))
        .collect()
    };
    let cards = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .split(chunks[2]);
    for ((title, value), rect) in stats.into_iter().zip(cards.iter()) {
        frame.render_widget(
            Paragraph::new(value)
                .style(
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )
                .block(theme.block(format!(" {title} "))),
            *rect,
        );
    }
    let status = STATUSES[app.status_index];
    let search = if app.searching {
        let (line, column) = input_editor(&app.draft, app.draft_cursor, chunks[3].width);
        frame.set_cursor_position((chunks[3].x + column, chunks[3].y));
        line
    } else if let Some(id) = &app.focused_load {
        Line::from(format!(
            " {}{} Shipment from {}: {id}",
            app.load_parties.summary(),
            if status.is_empty() {
                String::new()
            } else {
                format!("[{status}] ")
            },
            app.related_from.map(|view| view.title()).unwrap_or("Loads")
        ))
    } else if app.view == View::Integrations {
        Line::from(format!(
            " [{} · {}] / {}",
            if app.queue_attention {
                "Needs attention"
            } else {
                "All states"
            },
            app.queue_direction.title(),
            if app.query.is_empty() {
                "Search with /"
            } else {
                &app.query
            }
        ))
    } else if app.view.financial() || app.view == View::Operations {
        Line::from(format!(
            " [{}] / {}",
            if app.view == View::Operations {
                app.operations_scope.title()
            } else {
                app.balance_filter.title(app.view)
            },
            if app.query.is_empty() {
                "Search with /"
            } else {
                &app.query
            }
        ))
    } else {
        Line::from(format!(
            " {}{} / {}",
            app.load_parties.summary(),
            if status.is_empty() {
                String::new()
            } else {
                format!("[{status}]")
            },
            if app.query.is_empty() {
                "Search with /"
            } else {
                &app.query
            }
        ))
    };
    let state = if app.loading.is_some() || app.needs_fetch {
        if app.updated.is_some() {
            "Refreshing · showing previous snapshot…".into()
        } else {
            "Loading…".into()
        }
    } else if let Some(error) = &app.error {
        format!(
            "{}{}",
            if app.updated.is_some() {
                "STALE · "
            } else {
                ""
            },
            error
        )
    } else {
        format!(
            "{} shown{} · {} · {}s old",
            app.visible.len(),
            if app.catalog() {
                format!(
                    " / {} matching · Page {}{}",
                    app.catalog_total
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "—".into()),
                    app.cursors.len(),
                    if app.next_cursor.is_some() {
                        " · more available"
                    } else {
                        ""
                    }
                )
            } else if app.paged() {
                format!(
                    " · Page {}{}",
                    app.cursors.len(),
                    if app.next_cursor.is_some() {
                        " · more available"
                    } else {
                        ""
                    }
                )
            } else {
                String::new()
            },
            if app.paged() {
                "server search"
            } else {
                "workspace search"
            },
            app.updated.map(|v| v.elapsed().as_secs()).unwrap_or(0)
        )
    };
    let refresh = match app.refresh_interval {
        None => "manual".into(),
        Some(interval) => match app.refresh_pause() {
            Some(reason) => format!("auto paused: {reason}"),
            None => format!("auto {}s", interval.as_secs()),
        },
    };
    let state = format!(
        "{}{refresh} · {}",
        if app.stale() { "STALE · " } else { "" },
        state
    );
    frame.render_widget(
        Paragraph::new(vec![
            search.style(Style::default().fg(theme.accent)),
            Line::from(Span::styled(
                clean(&state),
                Style::default().fg(if app.error.is_some() || app.stale() {
                    theme.warning
                } else {
                    theme.muted
                }),
            )),
        ]),
        chunks[3],
    );
    let body = if area.width >= 116 {
        Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(chunks[4])
    } else {
        Layout::horizontal([Constraint::Percentage(100)]).split(chunks[4])
    };
    table(frame, app, theme, body[0]);
    if body.len() > 1 {
        detail(frame, app, theme, body[1], false);
    }
    let shortcuts = if app.searching {
        " ←→ Home/End  Ctrl+U clear  Enter apply  Esc cancels"
    } else if app.pending.is_some() || app.write_lost {
        " c check original  x stop review  i status  ? q"
    } else if app.error.is_some() {
        " Ctrl+R retry page  r first page  i status  ? q"
    } else if app.related_from.is_some() {
        " b return  v evidence  Enter details  r refresh  ? q"
    } else if app.view == View::Operations {
        " a work actions  f work view  n/p pages  ? q"
    } else if app.view == View::Integrations && app.catalog() {
        " f attention  d direction  n/p pages  r refresh  ? q"
    } else if app.view.financial() && app.catalog() {
        " f balance filter  n/p pages  r refresh  ? q"
    } else if app.view == View::Integrations {
        " f attention  d direction  r refresh  ? q"
    } else if app.view.financial() {
        " f balance filter  r refresh  R auto  ? q"
    } else if app.has_filters() {
        " Backspace clear filters  / edit  r refresh  ? q"
    } else if area.width >= 100 {
        " s status n/p page r refresh R auto a actions c/x recovery i status ? q quit"
    } else {
        match app.view {
            View::Loads if crate::actions::capability(&app.identity, "operations.write") => {
                " a actions  s status  n/p page  r refresh  ? q"
            }
            View::Loads => " s status  n/p page  r refresh  i status  ? q",
            View::Operations => " n/p page  r refresh  R auto  i status  ? q",
            _ => " r refresh  R auto  i status  ? help  q quit",
        }
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                if app.write_busy {
                    " Sending original request…".into()
                } else {
                    app.write_notice.clone().unwrap_or_else(|| {
                        if app.searching {
                            format!(
                                " Search: {}/200 bytes · Backspace/Delete remove",
                                app.draft.len()
                            )
                        } else if let Some(from) = app.related_from {
                            format!(" b returns to {} · / changes search", from.title())
                        } else if app.view == View::Loads {
                            " ↑↓ select  / search  v evidence  Enter details".into()
                        } else if app.view == View::Integrations && app.has_filters() {
                            " Backspace clear filters  / search  Enter details".into()
                        } else if (app.view.financial() || app.view == View::Operations)
                            && app.has_filters()
                        {
                            if row_link_available(app) {
                                " Backspace clear  o shipment  Enter details".into()
                            } else {
                                " Backspace clear filters  / search  Enter details".into()
                            }
                        } else if row_link_available(app) {
                            " o open shipment  / search  Enter details".into()
                        } else if area.width < 100 {
                            " Tab view  ↑↓ select  / search  Enter details".into()
                        } else {
                            " 1–5 / Tab view   ↑↓ / j k select   / search   v evidence  Enter details".into()
                        }
                    })
                },
                Style::default().fg(if app.pending.is_some() {
                    theme.warning
                } else {
                    theme.muted
                }),
            )),
            Line::from(Span::styled(shortcuts, Style::default().fg(theme.muted))),
        ]),
        chunks[5],
    );
    if app.details {
        let rect = centered(area, 94, area.height.saturating_sub(4));
        frame.render_widget(Clear, rect);
        detail(frame, app, theme, rect, true);
    }
    if let Some(panel) = &app.evidence {
        app.evidence_rect = crate::evidence_ui::render(
            frame,
            panel,
            theme,
            app.kitty_graphics,
            app.kitty_image_id,
            app.color_preview,
            app.image_cell,
        );
    }
    if app.connection {
        connection(frame, app, theme, area, demo, origin);
    }
    if app.help {
        let rect = centered(area, 82, 30);
        frame.render_widget(Clear, rect);
        let help="↑ / ↓ scroll · ? / Enter / Esc close\n\nCtrl+P                  Search navigation commands\nLoads palette           Selected customer / carrier\n1–5 / Tab / Shift+Tab   Switch workspace\n↑ ↓ / j k               Select a row\nHome / End / g / G       First / last row\nPageUp / PageDown        Move ten rows\n/                       Search; Enter applies\n← → / Home / End        Move the search caret\nBackspace / Delete      Delete a search grapheme\nCtrl+U                  Clear the search draft\nBackspace (table)       Clear applied filters\ns                       Cycle shipment status\nf (Operations)          Cycle server work view\nf (Billing/Payables)     Cycle balance filter\nf (Queues)              Toggle attention items\nd (Queues)              All / outbound / inbound\nn / p                   Server pages; retain each page row\nr                       Refresh from the first page\nCtrl+R                  Reread current page; keep selection\nR                       Toggle automatic reads (30s)\na                       Review shipment / work actions\nc / x                   Check / stop uncertain original\ni                       Connection and recovery status\no                       Open the related shipment\nb                       Return from a linked shipment\nv                       Shipment evidence / image preview\nEnter                   Expand details; ↑ ↓ scroll\n← → / h l               Adjacent record in details\nHome / End / PageUp/Down Scroll an open panel\nq / Ctrl+C              Quit and restore terminal\n\nEach workspace remembers its search, page and row.\nReturning reads fresh data; r starts at page one.\nCompact rows retain routes and financial amounts.\nSTALE marks old data; refresh before a new action.\nAutomatic reads pause during editing, inspection,\nuncertain saves and errors. They never send a command.\n\nLive saves require encrypted recovery. Restart with\n--connect to enter hidden credentials and recovery key.\nKeep the same key and directory across restarts.\nDemo saves last only for the process lifetime.\n\nGhostty: Unicode/RGB; no custom font needed.\nUse --light or --no-color to match your terminal.";
        scroll_panel(
            frame,
            app,
            theme,
            rect,
            " Keyboard · ↑↓ scroll ".into(),
            Paragraph::new(help).wrap(Wrap { trim: false }),
        );
    }
    if app.palette.is_some() {
        command_palette(frame, app, theme, area);
    }
    if let Some(dialog) = &app.action_dialog {
        app.action_viewport_ready = action_dialog(frame, app, dialog, theme, area);
    }
    if app.quit_confirm {
        let rect = centered(area, 88, 16);
        frame.render_widget(Clear, rect);
        let key = app
            .pending
            .as_ref()
            .map(|p| p.key.as_str())
            .unwrap_or("unavailable after access closed");
        let recovery = if app.recovery_durable {
            "Live commands are sent only after encrypted recovery is synced.\nReopen with the same API, operator, directory and recovery key.\nRestoration sends nothing automatically. Keep the key safe."
        } else {
            "Demo recovery is held only in this process.\nClosing loses its original body/key and recovery controls."
        };
        frame.render_widget(Paragraph::new(format!("An original save may already have committed.\n\n{recovery}\n\nRequest key: {key}\n\nEsc keeps this session open.\nUppercase Q exits this session.")).wrap(Wrap{trim:false}).style(Style::default().fg(theme.warning).bg(theme.background)).block(theme.block(" Unresolved original save ")),rect);
    }
}

fn command_palette(frame: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let choices = app.palette_choices();
    let Some(palette) = app.palette.as_mut() else {
        return;
    };
    let rect = centered(area, 76, 20);
    let block = theme.block(" Commands · Ctrl+P / Esc close ");
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        block.style(Style::default().bg(theme.background).fg(theme.foreground)),
        rect,
    );
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let (line, column) = input_editor(&palette.query, palette.query.len(), chunks[0].width);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(theme.accent)),
        chunks[0],
    );
    frame.set_cursor_position((chunks[0].x + column, chunks[0].y));
    palette.selected = palette.selected.min(choices.len().saturating_sub(1));
    palette
        .list
        .select((!choices.is_empty()).then_some(palette.selected));
    if choices.is_empty() {
        frame.render_widget(
            Paragraph::new("No commands match. Ctrl+U clears search.")
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.muted)),
            chunks[1],
        );
    } else {
        let rows = choices.iter().map(|choice| {
            ListItem::new(format!(
                "{}{}",
                choice.title,
                if !choice.enabled {
                    " (unavailable)"
                } else if choice.active {
                    " [active]"
                } else {
                    ""
                }
            ))
            .style(Style::default().fg(if choice.enabled {
                theme.foreground
            } else {
                theme.muted
            }))
        });
        frame.render_stateful_widget(
            List::new(rows).highlight_symbol("› ").highlight_style(
                Style::default()
                    .bg(theme.selection)
                    .add_modifier(Modifier::BOLD),
            ),
            chunks[1],
            &mut palette.list,
        );
    }
    frame.render_widget(
        Paragraph::new(format!(
            " ↑↓ choose  Enter open  {}/{}",
            if choices.is_empty() {
                0
            } else {
                palette.selected + 1
            },
            choices.len()
        ))
        .style(Style::default().fg(theme.muted)),
        chunks[2],
    );
}

fn scroll_panel(
    frame: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    title: String,
    content: Paragraph<'_>,
) {
    let block = theme.block(title);
    let inner = block.inner(area);
    let lines = content.line_count(inner.width);
    app.detail_max =
        u16::try_from(lines.saturating_sub(usize::from(inner.height))).unwrap_or(u16::MAX);
    app.detail_scroll = app.detail_scroll.min(app.detail_max);
    app.detail_page = inner.height.saturating_sub(1).max(1);
    let first = if lines == 0 {
        0
    } else {
        usize::from(app.detail_scroll) + 1
    };
    let last = (usize::from(app.detail_scroll) + usize::from(inner.height)).min(lines);
    let progress = format!(" {first}–{last}/{lines} lines · PgUp/Dn Home/End ");
    frame.render_widget(
        content
            .scroll((app.detail_scroll, 0))
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(block.title_bottom(progress)),
        area,
    );
}

fn connection(
    frame: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    demo: bool,
    origin: &str,
) {
    let rect = centered(area, 92, area.height.saturating_sub(2));
    let refresh = app
        .refresh_interval
        .map(|v| format!("Every {} seconds", v.as_secs()))
        .unwrap_or_else(|| "Manual".into());
    let freshness = app
        .updated
        .map(|at| {
            format!(
                "{} · {} seconds old",
                if app.stale() {
                    "STALE — refresh before actions"
                } else {
                    "Current snapshot"
                },
                at.elapsed().as_secs()
            )
        })
        .unwrap_or_else(|| "No verified snapshot yet".into());
    let saves = if demo {
        "Synthetic demo; recovery lasts for this process".into()
    } else if let Some(error) = &app.recovery_error {
        error.clone()
    } else if app.pending.is_some() {
        "Original save needs explicit recovery; c checks, x reviews a stop".into()
    } else if app.recovery_durable {
        "Encrypted recovery ready for this verified operator".into()
    } else {
        "Recovery storage has not been verified".into()
    };
    let mut lines = vec![
        format!(
            "Mode: {}",
            if demo {
                "Synthetic demo — no server or credential"
            } else {
                "Staff API"
            }
        ),
        format!("API: {}", if demo { "Not connected" } else { origin }),
        format!("Tenant: {}", text(&app.identity["tenant_id"])),
        format!("Operator: {}", text(&app.identity["access"]["subject"])),
        format!("Role: {}", text(&app.identity["access"]["role"])),
        format!(
            "Read target: {} · {}",
            app.view.title(),
            if app.paged() {
                format!("page {}", app.cursors.len())
            } else {
                "workspace snapshot".into()
            }
        ),
        format!(
            "Read status: {}",
            if app.loading == Some(app.generation) {
                "Reading current view"
            } else if app.needs_fetch {
                "Read requested"
            } else if app.loading.is_some() {
                "Finishing previous read"
            } else {
                "Idle"
            }
        ),
        String::new(),
        format!("Data: {freshness}"),
        format!("Read refresh: {refresh}"),
        format!("Refresh pause: {}", app.refresh_pause().unwrap_or("None")),
        format!(
            "Connection: {}",
            app.error.as_deref().unwrap_or("No reported read error")
        ),
        String::new(),
        format!("Save recovery: {saves}"),
        format!(
            "Latest save message: {}",
            app.write_notice.as_deref().unwrap_or("None")
        ),
        String::new(),
        "Ctrl+R rereads this page. r starts at page one.".into(),
        "R toggles automatic reads.".into(),
        "Automatic reads pause for editing, inspection, recovery or errors.".into(),
        "To change connection, restart with --connect. Secrets are hidden and remain in memory."
            .into(),
        "Keep the same recovery key and directory across restarts.".into(),
        String::new(),
        "↑ / ↓ scroll · i / Enter / Esc close".into(),
    ];
    if app.view == View::Loads {
        lines.splice(
            8..8,
            crate::parties::Party::ALL.map(|party| {
                format!(
                    "{} filter: {}",
                    if party == crate::parties::Party::Customer {
                        "Customer"
                    } else {
                        "Carrier"
                    },
                    app.load_parties.get(party).unwrap_or("All")
                )
            }),
        );
    }
    frame.render_widget(Clear, rect);
    scroll_panel(
        frame,
        app,
        theme,
        rect,
        " Connection & recovery · Ctrl+R reread ".into(),
        Paragraph::new(clean_lines(&lines)).wrap(Wrap { trim: false }),
    );
}
fn clean_lines(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| clean(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn action_dialog(
    frame: &mut Frame,
    app: &App,
    dialog: &crate::actions::Dialog,
    theme: &Theme,
    area: Rect,
) -> bool {
    if dialog.target.work.is_some() {
        return crate::work_ui::action(frame, app, dialog, theme, area);
    }
    use crate::actions::{Kind, Stage};
    let rect = centered(area, 94, area.height.saturating_sub(2));
    frame.render_widget(Clear, rect);
    if !app.action_viewport_ready {
        frame.render_widget(Paragraph::new("Resize to at least 72 columns × 22 rows to review an action.\n\nConfirmation is disabled at this size. Esc closes.").wrap(Wrap{trim:false}).style(Style::default().fg(theme.warning).bg(theme.background)).block(theme.block(" Shipment action ")),rect);
        return false;
    }
    let mut lines = vec![
        format!("{} · {}", dialog.target.reference, dialog.target.id),
        format!(
            "Reviewed version {} · {}",
            dialog.target.version, dialog.target.status
        ),
        format!(
            "Workspace {} · {}",
            clean(&dialog.owner.tenant),
            clean(&dialog.owner.subject)
        ),
        String::new(),
    ];
    match dialog.stage {
        Stage::Choose => {
            for (i, kind) in Kind::choices(&dialog.target).iter().enumerate() {
                lines.push(format!(
                    "{}  {}{}",
                    i + 1,
                    kind.title(),
                    if kind.allowed(&dialog.target) {
                        ""
                    } else {
                        " (unavailable)"
                    }
                ));
            }
            lines.extend([
                String::new(),
                "Choose 1, 2 or 3. Esc closes without saving.".into(),
            ]);
        }
        Stage::Time => unreachable!("Shipment actions have no time step"),
        Stage::Edit | Stage::Review => {
            lines.push(dialog.kind.title().into());
            if dialog.kind == Kind::Exception {
                if let Some(exception) = &dialog.target.exception {
                    lines.push(format!(
                        "Existing exception (replaced by this note): {exception}"
                    ));
                }
            }
            if dialog.kind == Kind::Resolve {
                lines.push(format!(
                    "Current exception: {}",
                    dialog.target.exception.as_deref().unwrap_or("—")
                ));
            } else {
                lines.push(
                    if dialog.kind == Kind::Location {
                        "Actual reported location (max 200 UTF-8 bytes)"
                    } else {
                        "Actual exception note (max 200 UTF-8 bytes)"
                    }
                    .into(),
                );
                lines.push(format!(
                    "{}{}",
                    dialog.input,
                    if dialog.stage == Stage::Edit {
                        "▏"
                    } else {
                        ""
                    }
                ));
            }
            lines.push(String::new());
            lines.push(
                if dialog.stage == Stage::Edit {
                    "Enter reviews · Esc discards this draft"
                } else {
                    "y confirms and saves · Esc discards this draft"
                }
                .into(),
            );
            lines.push("The API rechecks the reviewed version and your permissions.".into());
        }
        Stage::StopEdit | Stage::StopReview => {
            lines.push("Resolve the uncertain original save".into());
            lines.push(
                "If committed, retain its original result. Otherwise prevent it applying.".into(),
            );
            lines.push("This does not undo an already committed change.".into());
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
    if let Some(error) = &dialog.error {
        lines.push(String::new());
        lines.push(error.clone());
    }
    let widget = Paragraph::new(lines.join("\n"))
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .block(theme.block(" Reviewed shipment action "));
    let fits = widget.line_count(rect.width) <= usize::from(rect.height);
    if fits {
        frame.render_widget(widget, rect);
    } else {
        frame.render_widget(
            Paragraph::new(
                "This review does not fit. Enlarge the window before confirming. Esc closes.",
            )
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.warning).bg(theme.background))
            .block(theme.block(" Shipment review requires more space ")),
            rect,
        );
    }
    fits
}
fn table(frame: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let compact = area.width < 90;
    let financial = matches!(app.view, View::Billing | View::Payables);
    let headers = if compact {
        match app.view {
            View::Loads => vec!["Load / status", "Origin → destination", "Carrier"],
            View::Operations => vec!["Load / owner", "Priority / deadline", "Issue"],
            View::Billing => vec!["Invoice / status", "Customer", "Amount / balance"],
            View::Payables => vec!["Load / status", "Carrier", "Total / balance"],
            View::Integrations => vec!["Load / destination", "Direction / status", "Attempts"],
        }
    } else {
        app.view.headers()
    };
    let rows = app.visible.iter().map(|index| {
        let row = &app.rows[*index];
        if compact {
            let pair = |a: usize, b: usize| format!("{}\n{}", row.cells[a], row.cells[b]);
            let values = match app.view {
                View::Loads => vec![
                    pair(0, 1),
                    format!("{}\n→ {}", row.cells[2], row.cells[3]),
                    row.cells[4].clone(),
                ],
                View::Operations => vec![pair(0, 3), pair(1, 4), row.cells[2].clone()],
                View::Billing | View::Payables => {
                    vec![pair(0, 1), row.cells[2].clone(), pair(3, 4)]
                }
                View::Integrations => vec![pair(0, 3), pair(2, 1), row.cells[4].clone()],
            };
            Row::new(values.into_iter().enumerate().map(|(i, value)| {
                let text = ratatui::text::Text::from(value);
                let text = if financial && i == 2 {
                    text.alignment(ratatui::layout::Alignment::Right)
                } else {
                    text
                };
                Cell::from(text).style(if i == 0 {
                    theme.status(&row.status)
                } else {
                    Style::default()
                })
            }))
            .height(2)
        } else {
            Row::new(row.cells.iter().enumerate().map(|(i, v)| {
                Cell::from(v.clone()).style(if i == 1 {
                    theme.status(&row.status)
                } else {
                    Style::default()
                })
            }))
        }
    });
    let widths = if compact && financial {
        vec![
            Constraint::Percentage(32),
            Constraint::Min(7),
            Constraint::Length(24),
        ]
    } else if compact {
        vec![
            Constraint::Percentage(29),
            Constraint::Percentage(46),
            Constraint::Percentage(25),
        ]
    } else {
        vec![
            Constraint::Percentage(19),
            Constraint::Percentage(18),
            Constraint::Percentage(23),
            Constraint::Percentage(23),
            Constraint::Percentage(17),
        ]
    };
    let widget = Table::new(rows, widths)
        .header(
            Row::new(headers)
                .style(Style::default().fg(theme.muted))
                .bottom_margin(1),
        )
        .block(theme.block(format!(
            " {} · {}/{}{} ",
            app.view.title(),
            if app.visible.is_empty() {
                0
            } else {
                app.selected + 1
            },
            app.visible.len(),
            if compact { " · Enter full record" } else { "" }
        )))
        .column_spacing(1)
        .row_highlight_style(
            Style::default()
                .bg(theme.selection)
                .fg(theme.accent)
                .add_modifier(
                    Modifier::BOLD
                        | if theme.background == Color::Reset {
                            Modifier::REVERSED
                        } else {
                            Modifier::empty()
                        },
                ),
        )
        .highlight_symbol("› ");
    if app.visible.is_empty() {
        // Rendering a loading/empty table resets its offset. Keep the remembered
        // viewport separate until the fresh records arrive.
        frame.render_stateful_widget(widget, area, &mut ratatui::widgets::TableState::default());
    } else {
        app.table.select(Some(app.selected));
        frame.render_stateful_widget(widget, area, &mut app.table);
    }
    if app.visible.is_empty() {
        let rect = Rect::new(
            area.x + 2,
            area.y + 4,
            area.width.saturating_sub(4),
            area.height.saturating_sub(5),
        );
        frame.render_widget(
            Paragraph::new(if app.loading.is_some() || app.needs_fetch {
                "Fetching freight records…"
            } else if app.denied {
                "Access denied. Press r after checking your credential."
            } else if app.error.is_some() {
                "Records unavailable. Ctrl+R retries this page; r restarts."
            } else if app.focused_load.is_some() && app.next_cursor.is_some() {
                "Linked shipment not on this page.\nn checks the next page; b returns."
            } else if app.focused_load.is_some() {
                "Linked shipment unavailable in this result. r refreshes; b returns."
            } else if app.has_filters() {
                "No matching records. Backspace clears filters; / edits search."
            } else {
                "No records on this page. Press r to refresh."
            })
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.muted)),
            rect,
        );
    }
}
fn detail(frame: &mut Frame, app: &mut App, theme: &Theme, area: Rect, expanded: bool) {
    let mut lines = vec![];
    if let Some(row) = app.current() {
        lines.push(Line::from(Span::styled(
            row.cells.first().cloned().unwrap_or_default(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        for (key, value) in &row.details {
            if value == "—" {
                continue;
            }
            lines.push(Line::from(Span::styled(
                key.clone(),
                Style::default().fg(theme.muted),
            )));
            lines.push(Line::from(value.clone()));
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            "Record ID",
            Style::default().fg(theme.muted),
        )));
        lines.push(Line::from(row.id.clone()));
    } else {
        lines.push(Line::from("Select a record to inspect."));
    }
    let content = Paragraph::new(lines).wrap(Wrap { trim: false });
    if expanded {
        let navigation = if row_link_available(app) {
            "o shipment"
        } else if app.related_from.is_some() {
            "b return"
        } else {
            "←→ record"
        };
        let title = format!(
            " Details {}/{} · {navigation} · Esc close ",
            if app.visible.is_empty() {
                0
            } else {
                app.selected + 1
            },
            app.visible.len()
        );
        scroll_panel(frame, app, theme, area, title, content);
    } else {
        frame.render_widget(
            content
                .style(Style::default().bg(theme.background).fg(theme.foreground))
                .block(theme.block(" Details · Enter expand ")),
            area,
        );
    }
}
fn row_link_available(app: &App) -> bool {
    app.view != View::Loads && app.current().is_some_and(|row| row.related_load.is_some())
}
