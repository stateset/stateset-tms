//! Session-local cursor positions, never cached read snapshots.
use crate::{
    actions::Owner,
    model::{App, Record},
};
use ratatui::widgets::TableState;

#[derive(Clone, Default)]
pub(crate) struct PagePosition {
    cursor: Option<String>,
    selected_id: Option<String>,
    table: TableState,
}
impl App {
    pub(crate) fn remember_page_position(&mut self) {
        if !self.paged() || self.denied || Owner::from_identity(&self.identity).is_none() {
            return;
        }
        let index = self.cursors.len().saturating_sub(1);
        let selected_id = self
            .current()
            .map(Record::selection_key)
            .or_else(|| self.selection_anchor.clone());
        let mut table = if selected_id.is_some() {
            self.table
        } else {
            TableState::default()
        };
        if selected_id.is_some() {
            table.select(Some(self.selected));
        }
        self.page_positions.resize_with(
            self.page_positions.len().max(index + 1),
            PagePosition::default,
        );
        self.page_positions[index] = PagePosition {
            cursor: self.cursors.last().cloned().flatten(),
            selected_id,
            table,
        };
    }
    pub(crate) fn turn_page(&mut self, forward: bool) {
        if !self.paged() || self.loading.is_some() {
            return;
        }
        if forward {
            let Some(cursor) = self.next_cursor.take() else {
                return;
            };
            self.remember_page_position();
            let index = self.cursors.len();
            // A changed next cursor starts a new branch. Positions from the
            // previous branch cannot select a record on this fresh page.
            if self
                .page_positions
                .get(index)
                .is_none_or(|p| p.cursor.as_ref() != Some(&cursor))
            {
                self.page_positions.truncate(index);
                self.page_positions.push(PagePosition {
                    cursor: Some(cursor.clone()),
                    ..PagePosition::default()
                });
            }
            self.cursors.push(Some(cursor));
        } else {
            if self.cursors.len() <= 1 {
                return;
            }
            self.remember_page_position();
            self.cursors.pop();
            self.next_cursor = None;
        }
        let index = self.cursors.len().saturating_sub(1);
        let cursor = self.cursors.last().cloned().flatten();
        let position = self
            .page_positions
            .get(index)
            .filter(|p| p.cursor == cursor);
        self.table = position.map(|p| p.table).unwrap_or_default();
        self.selection_anchor = position.and_then(|p| p.selected_id.clone());
        self.selected = self.table.selected().unwrap_or(0);
        self.detail_scroll = 0;
        self.rows.clear();
        self.visible.clear();
        self.updated = None;
        self.invalidate();
    }
}
