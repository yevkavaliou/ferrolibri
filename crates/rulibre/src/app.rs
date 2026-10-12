use std::io;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Rect},
    widgets::{ScrollbarState, TableState},
};

use rulibre_core::config;
use rulibre_core::device::{DeviceEvent, DeviceState};
use rulibre_core::jobs::{JobEvent, JobExecutor, JobHandle, JobOutput, JobRequest};
use rulibre_core::scanner::Book;

pub(crate) enum PendingKind {
    Scan,
    Metadata(Book, u64),
    Backends(Book),
    Convert(Book, rulibre_core::converter::Workspace),
    Send(Book),
}
pub(crate) struct PendingJob {
    pub handle: JobHandle,
    pub kind: PendingKind,
}

use super::views;
use super::views::convert::ConvertState;
use super::views::detail::DetailState;
use super::views::notification::NotificationState;
use super::views::setup::SetupState;

#[derive(PartialEq)]
pub(crate) enum Mode {
    Setup,
    Normal,
    Search,
    Detail,
    Convert,
}

#[derive(PartialEq)]
pub(crate) enum Focus {
    Table,
    Detail,
}

pub struct App {
    pub(crate) jobs: JobExecutor,
    pub(crate) pending: Vec<PendingJob>,
    metadata_generation: u64,
    pub(crate) all_books: Vec<Book>,
    pub(crate) filtered_books: Vec<Book>,
    pub(crate) table_state: TableState,
    pub(crate) scrollbar_state: ScrollbarState,
    pub(crate) mode: Mode,
    pub(crate) search_query: String,
    pub(crate) focus: Focus,
    pub(crate) table_area: Rect,
    pub(crate) setup: SetupState,
    pub(crate) detail: DetailState,
    pub(crate) convert: ConvertState,
    pub(crate) notification: NotificationState,
    pub(crate) device: DeviceState,
}

impl App {
    pub fn new(cfg: Option<config::Config>) -> Result<Self, rulibre_core::Error> {
        let mut app = Self {
            jobs: JobExecutor::new(2, 16).map_err(|e| {
                rulibre_core::Error::io("start workers", std::path::Path::new("."), e)
            })?,
            pending: Vec::new(),
            metadata_generation: 0,
            all_books: Vec::new(),
            filtered_books: Vec::new(),
            table_state: TableState::default(),
            scrollbar_state: ScrollbarState::new(0),
            mode: Mode::Setup,
            search_query: String::new(),
            focus: Focus::Table,
            table_area: Rect::default(),
            setup: SetupState::default(),
            detail: DetailState::default(),
            convert: ConvertState::default(),
            notification: NotificationState::default(),
            device: DeviceState::default(),
        };
        if let Some(cfg) = cfg
            && config::is_calibre_library(&cfg.library_path)?
        {
            app.start_scan(cfg.library_path);
        }
        Ok(app)
    }

    pub(crate) fn start_scan(&mut self, library: std::path::PathBuf) {
        if self
            .pending
            .iter()
            .any(|job| matches!(job.kind, PendingKind::Scan))
        {
            return;
        }
        match self.jobs.submit(JobRequest::Scan { library }) {
            Ok(handle) => {
                self.pending.push(PendingJob {
                    handle,
                    kind: PendingKind::Scan,
                });
                self.setup.error = "Scanning library…".into();
            }
            Err(error) => self.setup.error = format!("{error:?}"),
        }
    }

    pub(crate) fn poll_jobs(&mut self) {
        let mut index = 0;
        while index < self.pending.len() {
            for event in self.pending[index].handle.events.try_iter() {
                let message = match event {
                    JobEvent::Stage { stage, .. } => Some(format!("{stage:?}")),
                    JobEvent::CopyProgress {
                        completed, total, ..
                    } => Some(format!("Copied {completed}/{total} bytes")),
                    _ => None,
                };
                if let Some(message) = message {
                    self.notification.set(Ok(message));
                }
            }
            let result = match self.pending[index].handle.try_result() {
                Ok(result) => result.map_err(|e| e.to_string()),
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
                Err(_) => Err("Job worker disconnected".into()),
            };
            let job = self.pending.remove(index);
            match job.kind {
                PendingKind::Scan => match result {
                    Ok(JobOutput::Scan(report)) => {
                        if !report.warnings.is_empty() {
                            self.notification.set(Err(report
                                .warnings
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join("; ")));
                        }
                        self.all_books = report.books;
                        self.apply_filter();
                        self.setup.error.clear();
                        self.mode = Mode::Normal;
                    }
                    Err(error) => {
                        self.setup.error = error;
                        self.mode = Mode::Setup;
                    }
                    _ => self.setup.error = "Unexpected scan result".into(),
                },
                PendingKind::Metadata(book, generation) => {
                    let selected = self
                        .table_state
                        .selected()
                        .and_then(|i| self.filtered_books.get(i));
                    if generation != self.metadata_generation
                        || selected.map(|b| &b.id) != Some(&book.id)
                        || self.mode != Mode::Detail
                    {
                        continue;
                    }
                    match result {
                        Ok(JobOutput::Metadata(meta)) => self.detail.metadata = Some(meta),
                        Err(error) => self.notification.set(Err(error)),
                        _ => self
                            .notification
                            .set(Err("Unexpected metadata result".into())),
                    }
                }
                PendingKind::Backends(book) => {
                    let selected = self
                        .table_state
                        .selected()
                        .and_then(|i| self.filtered_books.get(i));
                    if self.convert.selection_job != Some(job.handle.id)
                        || self.mode != Mode::Convert
                        || selected.map(|b| &b.id) != Some(&book.id)
                    {
                        continue;
                    }
                    match result {
                        Ok(JobOutput::Backends(backends)) => {
                            self.convert.targets =
                                rulibre_core::converter::target_formats(&book.files, &backends)
                                    .into_iter()
                                    .map(|(format, tool)| (format.label().to_owned(), tool))
                                    .collect();
                            self.convert.message = if self.convert.targets.is_empty() {
                                Some(Err(backends
                                    .iter()
                                    .map(|b| format!("{}: {}", b.kind.program(), b.diagnostic))
                                    .collect::<Vec<_>>()
                                    .join("; ")))
                            } else {
                                None
                            };
                        }
                        Err(error) => self.convert.message = Some(Err(error)),
                        _ => self.convert.message = Some(Err("Unexpected backend result".into())),
                    }
                }
                PendingKind::Convert(book, workspace) => {
                    let message = match result {
                        Ok(JobOutput::Conversion(result)) => {
                            self.convert.workspaces.push(workspace);
                            Ok(format!(
                                "Temporary result: {} (removed on exit)",
                                result.output_path.display()
                            ))
                        }
                        Err(error) => Err(error),
                        _ => Err("Unexpected conversion result".into()),
                    };
                    let selected = self
                        .table_state
                        .selected()
                        .and_then(|i| self.filtered_books.get(i));
                    if self.convert.active == Some(job.handle.id)
                        && self.mode == Mode::Convert
                        && selected.map(|b| &b.id) == Some(&book.id)
                    {
                        self.convert.message = Some(message.clone());
                        if message.is_ok() {
                            self.convert.targets.clear();
                            self.mode = Mode::Normal;
                        }
                    }
                    if self.convert.active == Some(job.handle.id) {
                        self.convert.active = None;
                    }
                    self.notification.set(
                        message
                            .map(|m| format!("{}: {m}", book.title))
                            .map_err(|e| format!("{}: {e}", book.title)),
                    );
                }
                PendingKind::Send(book) => self.notification.set(match result {
                    Ok(JobOutput::Sent(path)) => {
                        Ok(format!("{}: Sent to {}", book.title, path.display()))
                    }
                    Err(error) => Err(format!("{}: {error}", book.title)),
                    _ => Err("Unexpected transfer result".into()),
                }),
            }
        }
    }

    pub fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        device_rx: mpsc::Receiver<DeviceEvent>,
    ) -> io::Result<()> {
        loop {
            self.poll_jobs();
            terminal.draw(|frame| self.draw(frame))?;

            // Process device events from background thread
            while let Ok(ev) = device_rx.try_recv() {
                let msg = self.device.handle_event(ev);
                self.notification.set(msg);
            }

            // Tick notification auto-clear (~3s at 250ms poll)
            self.notification.tick();

            // Poll with timeout so we can process device events between frames
            if !event::poll(Duration::from_millis(250))? {
                continue;
            }

            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if self.handle_key(key.code) {
                        return Ok(());
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        self.handle_click(mouse.column, mouse.row);
                    }
                    MouseEventKind::ScrollUp => {
                        self.handle_scroll(mouse.column, mouse.row, true);
                    }
                    MouseEventKind::ScrollDown => {
                        self.handle_scroll(mouse.column, mouse.row, false);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// Returns `true` if the app should quit.
    fn handle_key(&mut self, code: KeyCode) -> bool {
        match self.mode {
            Mode::Normal => match code {
                KeyCode::Char('q') | KeyCode::Esc => return true,
                KeyCode::Down | KeyCode::Char('s') => self.next(),
                KeyCode::Up | KeyCode::Char('w') => self.previous(),
                KeyCode::Char('/') => {
                    self.mode = Mode::Search;
                    self.search_query.clear();
                }
                KeyCode::Enter => self.open_detail(),
                KeyCode::Char('c') => views::convert::enter(self),
                KeyCode::Char('t') => self.send_to_device(),
                _ => {}
            },
            Mode::Setup => return views::setup::handle_key(self, code),
            Mode::Search => views::search::handle_key(self, code),
            Mode::Convert => views::convert::handle_key(self, code),
            Mode::Detail => views::detail::handle_key(self, code),
        }
        false
    }

    pub(crate) fn open_detail(&mut self) {
        let Some(idx) = self.table_state.selected() else {
            return;
        };
        let Some(book) = self.filtered_books.get(idx) else {
            return;
        };
        let book = book.clone();
        self.metadata_generation += 1;
        self.detail.metadata = None;
        for job in &self.pending {
            if matches!(job.kind, PendingKind::Metadata(..)) {
                job.handle.cancel();
            }
        }
        match self.jobs.submit(JobRequest::Metadata {
            book: book.path.clone(),
        }) {
            Ok(handle) => self.pending.push(PendingJob {
                handle,
                kind: PendingKind::Metadata(book, self.metadata_generation),
            }),
            Err(error) => self.notification.set(Err(format!("{error:?}"))),
        }
        self.detail.scroll = 0;
        self.mode = Mode::Detail;
    }

    pub(crate) fn enter_convert(&mut self) {
        views::convert::enter(self);
    }

    pub(crate) fn send_to_device(&mut self) {
        let Some(idx) = self.table_state.selected() else {
            return;
        };
        let Some(book) = self.filtered_books.get(idx) else {
            return;
        };
        let book = book.clone();
        let Some(device) = self.device.connected.clone() else {
            self.notification.set(Err("No device connected".into()));
            return;
        };
        match self.jobs.submit(JobRequest::Send {
            book: book.path.clone(),
            author: book.author.clone(),
            device,
            options: Default::default(),
        }) {
            Ok(handle) => self.pending.push(PendingJob {
                handle,
                kind: PendingKind::Send(book),
            }),
            Err(error) => self.notification.set(Err(format!("{error:?}"))),
        }
    }

    fn handle_click(&mut self, col: u16, row: u16) {
        if Self::is_in_area(col, row, self.table_area) {
            self.focus = Focus::Table;

            let area = self.table_area;
            // Account for border (1) + header (1) + header bottom_margin (1) = 3 rows offset
            let content_start = area.y + 3;
            if row < content_start {
                return;
            }

            let clicked_row = (row - content_start) as usize;
            let offset = self.table_state.offset();
            let idx = offset + clicked_row;

            if idx < self.filtered_books.len() {
                self.table_state.select(Some(idx));
                self.scrollbar_state = self.scrollbar_state.position(idx);
                self.open_detail();
            }
        } else if matches!(self.mode, Mode::Detail) && Self::is_in_area(col, row, self.detail.area)
        {
            self.focus = Focus::Detail;
        }
    }

    fn is_in_area(col: u16, row: u16, area: Rect) -> bool {
        col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
    }

    fn handle_scroll(&mut self, col: u16, row: u16, up: bool) {
        if Self::is_in_area(col, row, self.table_area) {
            if up {
                self.previous();
            } else {
                self.next();
            }
        } else if matches!(self.mode, Mode::Detail) && Self::is_in_area(col, row, self.detail.area)
        {
            if up {
                self.detail.scroll = self.detail.scroll.saturating_sub(1);
            } else {
                self.detail.scroll = self.detail.scroll.saturating_add(1);
            }
        }
    }

    pub(crate) fn apply_filter(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_books = self.all_books.clone();
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered_books = self
                .all_books
                .iter()
                .filter(|b| {
                    b.author.to_lowercase().contains(&query)
                        || b.title.to_lowercase().contains(&query)
                        || formats_label(b).to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
        }
        self.scrollbar_state = ScrollbarState::new(self.filtered_books.len().saturating_sub(1));
        if self.filtered_books.is_empty() {
            self.table_state.select(None);
        } else {
            self.table_state.select(Some(0));
            self.scrollbar_state = self.scrollbar_state.position(0);
        }
    }

    pub(crate) fn next(&mut self) {
        if self.filtered_books.is_empty() {
            return;
        }
        let i = self.table_state.selected().map_or(0, |i| {
            if i >= self.filtered_books.len() - 1 {
                0
            } else {
                i + 1
            }
        });
        self.table_state.select(Some(i));
        self.scrollbar_state = self.scrollbar_state.position(i);
    }

    pub(crate) fn previous(&mut self) {
        if self.filtered_books.is_empty() {
            return;
        }
        let i = self.table_state.selected().map_or(0, |i| {
            if i == 0 {
                self.filtered_books.len() - 1
            } else {
                i - 1
            }
        });
        self.table_state.select(Some(i));
        self.scrollbar_state = self.scrollbar_state.position(i);
    }

    fn draw(&mut self, frame: &mut Frame) {
        let chunks =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());

        match self.mode {
            Mode::Detail => {
                let cols =
                    Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                        .split(chunks[0]);
                views::table::draw(self, frame, cols[0]);
                views::detail::draw(self, frame, cols[1]);
            }
            Mode::Setup => {
                views::setup::draw(self, frame);
                return;
            }
            Mode::Convert => {
                views::table::draw(self, frame, chunks[0]);
                views::convert::draw(self, frame);
            }
            _ => {
                views::table::draw(self, frame, chunks[0]);
            }
        }

        views::table::draw_status_bar(self, frame, chunks[1]);
        views::notification::draw(self, frame);
    }
}

pub(crate) fn formats_label(book: &rulibre_core::book::Book) -> String {
    book.files
        .iter()
        .map(|f| f.format)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|f| f.label())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "../../rulibre-core/src/test_support.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::*;
    fn drain(app: &mut App) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !app.pending.is_empty() {
            app.poll_jobs();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn initial_scan_is_queued_and_completion_populates_table() {
        let fixture = fixtures::Fixture::new();
        let mut app = App::new(Some(config::Config {
            library_path: fixture.library.clone(),
        }))
        .unwrap();
        assert!(app.all_books.is_empty());
        assert_eq!(app.pending.len(), 1);
        drain(&mut app);
        assert_eq!(app.all_books.len(), 2);
        assert!(app.mode == Mode::Normal);
    }
    #[test]
    fn changing_selection_ignores_old_metadata_including_return_to_same_book() {
        let fixture = fixtures::Fixture::new();
        let mut app = App::new(Some(config::Config {
            library_path: fixture.library.clone(),
        }))
        .unwrap();
        drain(&mut app);
        app.table_state.select(Some(0));
        app.open_detail();
        app.table_state.select(Some(1));
        app.open_detail();
        drain(&mut app);
        assert_eq!(
            app.detail.metadata.as_ref().unwrap().title,
            app.filtered_books[1].title
        );
        app.table_state.select(Some(0));
        app.open_detail();
        app.table_state.select(Some(1));
        app.open_detail();
        app.table_state.select(Some(0));
        app.open_detail();
        drain(&mut app);
        assert_eq!(
            app.detail.metadata.as_ref().unwrap().title,
            app.filtered_books[0].title
        );
    }
    #[test]
    fn scan_failure_returns_to_setup_with_error() {
        let fixture = fixtures::Fixture::new();
        let mut app = App::new(None).unwrap();
        app.start_scan(fixture.root.join("missing"));
        drain(&mut app);
        assert!(app.mode == Mode::Setup);
        assert!(!app.setup.error.is_empty());
    }
}
