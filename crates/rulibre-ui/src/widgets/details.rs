use crate::{
    app::{Message, State},
    services::Services,
    theme,
};
use iced::{
    Element, Fill,
    widget::{button, column, container, image, row, scrollable, text},
};
use rulibre_core::{
    book::{Book, Format},
    converter::{self, Backend},
    metadata::Metadata,
};
use std::path::PathBuf;

pub const TARGETS: [Format; 7] = [
    Format::Epub,
    Format::Kepub,
    Format::Azw3,
    Format::Mobi,
    Format::Pdf,
    Format::Docx,
    Format::Txt,
];

#[derive(Clone)]
pub struct ConversionBatch {
    pub library: PathBuf,
    pub books: Vec<Book>,
    pub target: Format,
}
impl ConversionBatch {
    pub fn enqueue(self, services: &dyn Services, output: PathBuf) {
        for book in self.books {
            services.convert(
                format!("{} → {}", book.title, self.target.label()),
                self.library.clone(),
                output.clone(),
                book.files,
                self.target,
            );
        }
    }
}
pub fn selected(state: &State) -> Vec<Book> {
    state
        .catalog
        .order
        .iter()
        .filter(|id| state.selection.selected.contains(id))
        .filter_map(|id| state.catalog.get(id).map(|entry| entry.book.clone()))
        .collect()
}
pub fn targets(books: &[Book], backends: &[Backend]) -> Vec<Format> {
    if books.is_empty() {
        return Vec::new();
    }
    TARGETS
        .into_iter()
        .filter(|target| {
            books.iter().all(|book| {
                converter::target_formats(&book.files, backends)
                    .iter()
                    .any(|(format, _)| format == target)
            })
        })
        .collect()
}
pub fn batch(state: &State, target: Format) -> Option<ConversionBatch> {
    let books = selected(state);
    if !targets(&books, &state.backends).contains(&target) {
        return None;
    }
    Some(ConversionBatch {
        library: state.library.clone()?,
        books,
        target,
    })
}
pub fn unavailable(book: &Book, target: Format, backends: &[Backend]) -> String {
    if book.files.iter().any(|file| file.format == target) {
        return "Already available".into();
    }
    converter::plan_conversion(&book.files, target, backends)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default()
}
pub fn content_id() -> iced::widget::Id {
    iced::widget::Id::new("book-details-content")
}
pub fn view(state: &State) -> Element<'_, Message> {
    let books = selected(state);
    let mut content = column![].spacing(10).width(Fill);
    if books.is_empty() {
        return text("Select a book to see its details")
            .size(theme::font(state, 12.0))
            .color(theme::DARK.text_secondary)
            .into();
    }
    if books.len() == 1 {
        let book = &books[0];
        // The cover never outgrows the pane (its text area is 24 px narrower).
        let cover_width =
            (state.visible_settings().cover_size as f32).min(state.details_width() - 24.0);
        if let Some(cover) = state.covers.get(&book.id) {
            content = content.push(
                container(
                    image(cover.clone())
                        .width(cover_width)
                        .height(cover_width * 1.375)
                        .content_fit(iced::ContentFit::Contain),
                )
                .center_x(Fill),
            );
        } else {
            content = content.push(
                container(
                    text("No cover")
                        .size(theme::font(state, 12.0))
                        .color(theme::DARK.text_muted),
                )
                .height(100)
                .center_x(Fill)
                .center_y(100),
            );
        }
        let metadata = state
            .metadata
            .clone()
            .unwrap_or_default()
            .with_fallback(&book.title, &book.author);
        content = content.push(
            text(metadata.title.clone())
                .size(theme::font(state, 16.0))
                .width(Fill),
        );
        if let Some(error) = &state.metadata_error {
            content = content.push(super::tip(
                text("Metadata could not be read")
                    .size(theme::font(state, 11.0))
                    .color(theme::DARK.text_muted),
                text(error).size(theme::font(state, 12.0)),
                super::Position::Left,
            ));
        } else if state.metadata.is_none() {
            content = content.push(
                text("Loading metadata…")
                    .size(theme::font(state, 11.0))
                    .color(theme::DARK.text_muted),
            );
        }
        let details = fields(&metadata, book);
        if state.metadata.is_some()
            && metadata.description.is_empty()
            && details
                .iter()
                .all(|(label, _)| matches!(*label, "Author" | "Formats"))
        {
            content = content.push(
                text("This book has no further details. Calibre can add them.")
                    .size(theme::font(state, 11.0))
                    .color(theme::DARK.text_muted),
            );
        }
        for (label, value) in details {
            content = content.push(
                column![
                    text(label)
                        .size(theme::font(state, 10.0))
                        .color(theme::DARK.text_muted),
                    text(value).size(theme::font(state, 12.0)).width(Fill)
                ]
                .spacing(2),
            );
        }
        if !metadata.description.is_empty() {
            content = content.push(
                text(metadata.description)
                    .size(theme::font(state, 12.0))
                    .width(Fill),
            );
        }
    } else {
        content =
            content.push(text(format!("{} selected", books.len())).size(theme::font(state, 16.0)));
        for book in books.iter().take(5) {
            content = content.push(
                text(book.title.clone())
                    .size(theme::font(state, 12.0))
                    .width(Fill),
            );
        }
        if books.len() > 5 {
            content = content.push(
                text(format!("+ {} more", books.len() - 5))
                    .size(theme::font(state, 12.0))
                    .color(theme::DARK.text_muted),
            );
        }
    }
    scrollable(content)
        .id(content_id())
        .direction(theme::vbar())
        .style(theme::scroll(state))
        .height(Fill)
        .into()
}
pub fn fields(metadata: &Metadata, book: &Book) -> Vec<(&'static str, String)> {
    [
        ("Author", metadata.authors.join(", ")),
        ("Publisher", metadata.publisher.clone()),
        ("Year", metadata.date.chars().take(4).collect()),
        ("Language", metadata.language.clone()),
        (
            "Series",
            if metadata.series.is_empty() {
                String::new()
            } else if metadata.series_index.is_empty() {
                metadata.series.clone()
            } else {
                format!("{} · {}", metadata.series, metadata.series_index)
            },
        ),
        (
            "Formats",
            book.files
                .iter()
                .map(|f| f.format.label())
                .collect::<Vec<_>>()
                .join(", "),
        ),
        ("Rating", metadata.rating.clone()),
        (
            "Identifiers",
            metadata
                .identifiers
                .iter()
                .map(|(kind, value)| format!("{kind}: {value}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        ("Tags", metadata.subjects.join(", ")),
    ]
    .into_iter()
    .filter(|(_, value)| !value.trim().is_empty())
    .collect()
}
pub fn export_prompt(state: &State) -> Element<'_, Message> {
    container(column![
        text("Choose an export folder").size(theme::font(state, 18.0)),
        text("Converted books will be saved here. Choose a writable folder outside your Calibre library.").size(theme::font(state, 13.0)),
        text(state.conversion_export.as_ref().map(|path| path.display().to_string()).unwrap_or_else(|| "No folder selected".into())).size(theme::font(state, 12.0)),
        button("Browse…").style(theme::outline).on_press_maybe((!state.export_busy).then_some(Message::ExportBrowse)),
        row![button("Cancel").style(theme::quiet).on_press(Message::CancelConvert), button("Use folder").style(theme::outline).on_press_maybe((state.conversion_export.is_some() && !state.export_busy).then_some(Message::UseExport))].spacing(12),
    ].spacing(12)).padding(24).width(420).style(theme::card).into()
}
