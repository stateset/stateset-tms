//! Searchable navigation and inspection commands; business writes stay in their reviewed flow.
use crate::model::{clean, App, BalanceFilter, QueueDirection, View, STATUSES};
use crate::operations::Scope;
use crate::parties::Party;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub struct Palette {
    pub query: String,
    pub selected: usize,
    pub list: ListState,
}
impl Palette {
    pub fn insert(&mut self, value: &str) {
        for grapheme in clean(value).graphemes(true) {
            if self.query.len() + grapheme.len() > 200 {
                break;
            }
            self.query.push_str(grapheme);
        }
        self.selected = 0;
        self.list = ListState::default();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Workspace(View),
    Search,
    Refresh,
    RefreshPage,
    NextPage,
    PreviousPage,
    AutoReads,
    Details,
    Actions,
    Connection,
    Help,
    ClearFilters,
    Related,
    Return,
    Status(usize),
    SelectedParty(Party),
    ClearParty(Party),
    WorkScope(Scope),
    Balance(BalanceFilter),
    Attention(bool),
    Direction(QueueDirection),
}
pub struct Choice {
    pub command: Command,
    pub title: String,
    pub keywords: &'static str,
    pub enabled: bool,
    pub active: bool,
}
impl Choice {
    fn new(command: Command, title: impl Into<String>, keywords: &'static str) -> Self {
        Self {
            command,
            title: title.into(),
            keywords,
            enabled: true,
            active: false,
        }
    }
}
impl App {
    pub fn palette_choices(&self) -> Vec<Choice> {
        let mut choices = View::ALL
            .into_iter()
            .map(|view| {
                let keywords = match view {
                    View::Loads => "shipments catalog dispatch",
                    View::Operations => "issues follow-up",
                    View::Billing => "invoice customer refund",
                    View::Payables => "settlement carrier",
                    View::Integrations => "queues handoffs callbacks",
                };
                let mut choice = Choice::new(
                    Command::Workspace(view),
                    format!("Go to {}", view.title()),
                    keywords,
                );
                choice.active = self.view == view;
                choice
            })
            .collect::<Vec<_>>();
        choices.extend([
            Choice::new(Command::Search, "Search this workspace", "find query"),
            Choice::new(
                Command::Refresh,
                "Refresh from first page",
                "reload read retry",
            ),
            Choice::new(
                Command::RefreshPage,
                "Refresh current page",
                "reread retry connection",
            ),
            Choice::new(
                Command::AutoReads,
                "Toggle automatic reads",
                "refresh timer scheduled",
            ),
            Choice::new(
                Command::Details,
                "Inspect selected record",
                "details expand",
            ),
            Choice::new(
                Command::Actions,
                "Review selected actions",
                "shipment work claim ownership release note follow-up defer resume",
            ),
            Choice::new(Command::Related, "Open related shipment", "linked load"),
            Choice::new(
                Command::Return,
                "Return to source workspace",
                "back linked shipment",
            ),
            Choice::new(
                Command::ClearFilters,
                "Clear this workspace's filters",
                "reset search status balance direction",
            ),
            Choice::new(
                Command::Connection,
                "Inspect connection and recovery",
                "identity freshness status",
            ),
            Choice::new(Command::Help, "Open keyboard help", "shortcuts keys"),
        ]);
        for choice in &mut choices {
            choice.enabled = match choice.command {
                Command::Details => self.current().is_some(),
                Command::Actions => self.can_open_action(),
                Command::Related => {
                    self.view != View::Loads
                        && !self.denied
                        && crate::actions::Owner::from_identity(&self.identity).is_some()
                        && self.current().is_some_and(|r| r.related_load.is_some())
                }
                Command::Return => self.related_from.is_some(),
                Command::ClearFilters => self.has_filters(),
                _ => true,
            };
            if choice.command == Command::AutoReads {
                choice.active = self.refresh_interval.is_some();
            }
        }
        if self.paged() {
            let mut next = Choice::new(Command::NextPage, "Next server page", "forward next page");
            next.enabled = self.loading.is_none() && self.next_cursor.is_some();
            let mut previous = Choice::new(
                Command::PreviousPage,
                "Previous server page",
                "back previous page",
            );
            previous.enabled = self.loading.is_none() && self.cursors.len() > 1;
            choices.extend([next, previous]);
        }
        if self.view == View::Loads {
            for party in Party::ALL {
                let mut selected = Choice::new(
                    Command::SelectedParty(party),
                    format!(
                        "Loads for selected {}: {}",
                        party.title(),
                        self.current()
                            .and_then(|row| row.load_parties.get(party))
                            .unwrap_or("unavailable")
                    ),
                    "filter exact party",
                );
                selected.enabled = self.can_filter_selected_party(party);
                selected.active = self
                    .current()
                    .and_then(|row| row.load_parties.get(party))
                    .is_some_and(|id| self.load_parties.get(party) == Some(id));
                let mut clear = Choice::new(
                    Command::ClearParty(party),
                    format!(
                        "Clear {} filter: {}",
                        party.title(),
                        self.load_parties.get(party).unwrap_or("none")
                    ),
                    "reset exact party",
                );
                clear.enabled =
                    self.verified_load_parties() && self.load_parties.get(party).is_some();
                clear.active = self.load_parties.get(party).is_some();
                choices.extend([selected, clear]);
            }
            choices.extend(STATUSES.into_iter().enumerate().map(|(index, status)| {
                let mut choice = Choice::new(
                    Command::Status(index),
                    format!(
                        "Shipment status: {}",
                        if status.is_empty() { "all" } else { status }
                    ),
                    "filter loads",
                );
                choice.active = self.status_index == index;
                choice
            }));
        } else if self.view == View::Operations {
            choices.extend(Scope::ALL.into_iter().map(|scope| {
                let mut choice = Choice::new(
                    Command::WorkScope(scope),
                    format!("Work view: {}", scope.title()),
                    if scope == Scope::Mine {
                        "operations queue filter mine my"
                    } else if scope == Scope::Inactive {
                        "operations queue filter resolved"
                    } else {
                        "operations queue filter"
                    },
                );
                choice.active = self.operations_scope == scope;
                choice
            }));
        } else if self.view.financial() {
            for filter in [
                BalanceFilter::All,
                BalanceFilter::Open,
                BalanceFilter::Overdue,
                BalanceFilter::Credits,
                BalanceFilter::Zero,
                BalanceFilter::Unknown,
            ] {
                if filter == BalanceFilter::Overdue && self.view != View::Billing {
                    continue;
                }
                let mut choice = Choice::new(
                    Command::Balance(filter),
                    format!("Balance: {}", filter.title(self.view)),
                    "filter finance",
                );
                choice.active = self.balance_filter == filter;
                choices.push(choice);
            }
        } else if self.view == View::Integrations {
            for (attention, title) in [
                (false, "Queue states: all"),
                (true, "Queue states: needs attention"),
            ] {
                let mut choice = Choice::new(
                    Command::Attention(attention),
                    title,
                    "filter retry held failed",
                );
                choice.active = self.queue_attention == attention;
                choices.push(choice);
            }
            for direction in [
                QueueDirection::All,
                QueueDirection::Outbound,
                QueueDirection::Inbound,
            ] {
                let mut choice = Choice::new(
                    Command::Direction(direction),
                    format!("Queue direction: {}", direction.title()),
                    "filter handoffs",
                );
                choice.active = self.queue_direction == direction;
                choices.push(choice);
            }
        }
        let query = self
            .palette
            .as_ref()
            .map(|p| p.query.to_lowercase())
            .unwrap_or_default();
        choices
            .into_iter()
            .filter(|choice| {
                let text = format!("{} {}", choice.title, choice.keywords).to_lowercase();
                query.split_whitespace().all(|word| text.contains(word))
            })
            .collect()
    }
    pub fn palette_key(&mut self, key: KeyEvent) {
        let choices = self.palette_choices();
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        palette.selected = palette.selected.min(choices.len().saturating_sub(1));
        match key.code {
            KeyCode::Esc => self.palette = None,
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if key.kind == KeyEventKind::Press {
                    self.palette = None;
                }
            }
            KeyCode::Enter if key.kind == KeyEventKind::Press => {
                if let Some(choice) = choices.get(palette.selected).filter(|c| c.enabled) {
                    let command = choice.command;
                    self.palette = None;
                    self.run_palette_command(command);
                }
            }
            KeyCode::Up => palette.selected = palette.selected.saturating_sub(1),
            KeyCode::Down => {
                palette.selected = (palette.selected + 1).min(choices.len().saturating_sub(1))
            }
            KeyCode::Home => palette.selected = 0,
            KeyCode::End => palette.selected = choices.len().saturating_sub(1),
            KeyCode::PageUp => palette.selected = palette.selected.saturating_sub(8),
            KeyCode::PageDown => {
                palette.selected = (palette.selected + 8).min(choices.len().saturating_sub(1))
            }
            KeyCode::Backspace => {
                if let Some((index, _)) = palette.query.grapheme_indices(true).next_back() {
                    palette.query.truncate(index);
                }
                palette.selected = 0;
                palette.list = ListState::default();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                *palette = Palette::default();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && !c.is_control() =>
            {
                palette.insert(&c.to_string())
            }
            _ => {}
        }
    }
    fn run_palette_command(&mut self, command: Command) {
        let key = match command {
            Command::Workspace(view) => KeyCode::Char(char::from(b'1' + view.index() as u8)),
            Command::Search => KeyCode::Char('/'),
            Command::Refresh => KeyCode::Char('r'),
            Command::RefreshPage => {
                self.refresh_read(false);
                return;
            }
            Command::NextPage => KeyCode::Char('n'),
            Command::PreviousPage => KeyCode::Char('p'),
            Command::AutoReads => KeyCode::Char('R'),
            Command::Details => KeyCode::Enter,
            Command::Actions => KeyCode::Char('a'),
            Command::Related => KeyCode::Char('o'),
            Command::Return => KeyCode::Char('b'),
            Command::ClearFilters => KeyCode::Backspace,
            Command::Connection => KeyCode::Char('i'),
            Command::Help => KeyCode::Char('?'),
            Command::WorkScope(scope) => {
                self.set_operations_scope(scope);
                return;
            }
            Command::SelectedParty(party) => {
                self.filter_selected_party(party);
                return;
            }
            Command::ClearParty(party) => {
                self.clear_party(party);
                return;
            }
            Command::Status(index) => {
                self.set_status(index);
                return;
            }
            Command::Balance(filter) => {
                self.set_balance_filter(filter);
                return;
            }
            Command::Attention(attention) => {
                if self.queue_attention != attention {
                    self.queue_attention = attention;
                    self.refilter_local();
                }
                return;
            }
            Command::Direction(direction) => {
                if self.queue_direction != direction {
                    self.queue_direction = direction;
                    self.refilter_local();
                }
                return;
            }
        };
        self.key(KeyEvent::new(key, KeyModifiers::NONE));
    }
}
