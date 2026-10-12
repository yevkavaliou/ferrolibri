use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use rulibre_core::converter;
use rulibre_core::scanner;

use crate::app::{App, Mode, PendingJob, PendingKind};
use rulibre_core::jobs::JobRequest;

#[derive(Default)]
pub(crate) struct ConvertState {
    pub(crate) selection_job: Option<rulibre_core::jobs::JobId>,
    pub(crate) active: Option<rulibre_core::jobs::JobId>,
    pub(crate) workspaces: Vec<converter::Workspace>,
    pub(crate) targets: Vec<(String, String)>,
    pub(crate) selected: usize,
    pub(crate) message: Option<Result<String, String>>,
}

pub fn draw(app: &App, frame: &mut Frame) {
    let area = frame.area();

    let book_title = app
        .table_state
        .selected()
        .and_then(|i| app.filtered_books.get(i))
        .map(|b| b.title.as_str())
        .unwrap_or("Unknown");

    let title = format!(" Convert: {book_title} ");

    // Box height: targets list + message line + hint line + borders + padding
    let list_len = app.convert.targets.len().max(1);
    let box_height = (list_len as u16 + 4).min(area.height.saturating_sub(2));
    let box_width = 50u16.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(box_width)) / 2;
    let y = area.y + (area.height.saturating_sub(box_height)) / 2;
    let box_area = Rect::new(x, y, box_width, box_height);

    frame.render_widget(Clear, box_area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::LightBlue));

    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    if app.convert.targets.is_empty() {
        // Only a message to display (error state)
        if let Some(ref result) = app.convert.message {
            let (msg, style) = match result {
                Ok(msg) => (msg.as_str(), Style::new().fg(Color::Green)),
                Err(msg) => (msg.as_str(), Style::new().fg(Color::Red)),
            };
            frame.render_widget(Paragraph::new(Span::styled(msg, style)), inner);
        }
        let hint_area = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            1,
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("esc", Style::new().fg(Color::Yellow).bold()),
                Span::raw(" close"),
            ])),
            hint_area,
        );
        return;
    }

    // Render target list
    let mut lines: Vec<Line> = Vec::new();
    for (i, (fmt, tool)) in app.convert.targets.iter().enumerate() {
        let marker = if i == app.convert.selected {
            "▶ "
        } else {
            "  "
        };
        let style = if i == app.convert.selected {
            Style::new().fg(Color::Yellow).bold()
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!("{marker}{fmt} [{tool}]"),
            style,
        )));
    }
    let list_area = Rect::new(inner.x, inner.y, inner.width, lines.len() as u16);
    frame.render_widget(Paragraph::new(lines), list_area);

    // Message below list
    if let Some(ref result) = app.convert.message {
        let msg_y = inner.y + list_area.height + 1;
        if msg_y < inner.y + inner.height {
            let (msg, style) = match result {
                Ok(msg) => (msg.as_str(), Style::new().fg(Color::Green)),
                Err(msg) => (msg.as_str(), Style::new().fg(Color::Red)),
            };
            let msg_area = Rect::new(inner.x, msg_y, inner.width, 1);
            frame.render_widget(Paragraph::new(Span::styled(msg, style)), msg_area);
        }
    }

    // Hint at bottom
    let hint_area = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("enter", Style::new().fg(Color::Yellow).bold()),
            Span::raw(" convert  "),
            Span::styled("esc", Style::new().fg(Color::Yellow).bold()),
            Span::raw(" cancel"),
        ])),
        hint_area,
    );
}

pub fn handle_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('q') => {
            if let Some(id) = app.convert.active.take()
                && let Some(job) = app.pending.iter().find(|job| job.handle.id == id)
            {
                job.handle.cancel();
            }
            app.mode = Mode::Normal;
            app.convert.selection_job = None;
            app.convert.targets.clear();
            app.convert.message = None;
        }
        KeyCode::Down | KeyCode::Char('s') => {
            if !app.convert.targets.is_empty() {
                app.convert.selected = (app.convert.selected + 1) % app.convert.targets.len();
            }
        }
        KeyCode::Up | KeyCode::Char('w') => {
            if !app.convert.targets.is_empty() {
                app.convert.selected = if app.convert.selected == 0 {
                    app.convert.targets.len() - 1
                } else {
                    app.convert.selected - 1
                };
            }
        }
        KeyCode::Enter => run(app),
        _ => {}
    }
}

pub fn enter(app: &mut App) {
    let Some(idx) = app.table_state.selected() else {
        return;
    };
    let Some(book) = app.filtered_books.get(idx) else {
        return;
    };

    let book = book.clone();
    app.convert.targets.clear();
    app.convert.selected = 0;
    app.convert.message = Some(Ok("Checking converters…".into()));
    app.mode = Mode::Convert;
    match app.jobs.submit(JobRequest::Backends) {
        Ok(handle) => {
            app.convert.selection_job = Some(handle.id);
            app.pending.push(PendingJob {
                handle,
                kind: PendingKind::Backends(book),
            });
        }
        Err(error) => app.convert.message = Some(Err(format!("{error:?}"))),
    }
}

pub fn run(app: &mut App) {
    if app.convert.targets.is_empty() {
        return;
    }

    let Some(idx) = app.table_state.selected() else {
        return;
    };
    let Some(book) = app.filtered_books.get(idx) else {
        return;
    };

    if app.convert.active.is_some() {
        return;
    }
    let book = book.clone();
    let Some((target, _)) = app.convert.targets.get(app.convert.selected) else {
        return;
    };
    let Some(target) = scanner::Format::from_label(target) else {
        return;
    };
    let result = (|| {
        let library = converter::library_root(&book.path).map_err(|e| e.to_string())?;
        let workspace = converter::Workspace::new(&library).map_err(|e| e.to_string())?;
        let handle = app
            .jobs
            .submit(JobRequest::ConvertFiles {
                library,
                output_dir: workspace.path().to_owned(),
                files: book.files.clone(),
                target,
            })
            .map_err(|e| format!("{e:?}"))?;
        Ok::<_, String>((handle, workspace))
    })();
    match result {
        Ok((handle, workspace)) => {
            app.convert.active = Some(handle.id);
            app.pending.push(PendingJob {
                handle,
                kind: PendingKind::Convert(book, workspace),
            });
            app.convert.message = Some(Ok("Waiting for conversion".into()));
        }
        Err(error) => app.convert.message = Some(Err(error)),
    }
}
