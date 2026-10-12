//! Fixed-height virtual list. Only viewport rows and a small overscan are built.
use crate::{
    app::{Filter, Message, ScanStatus, Screen, State},
    catalog::visible_slice_with_height,
    icons, theme,
};
use iced::{
    Alignment, Element, Fill, Length,
    widget::{
        Space, button, column, container, responsive, row, scrollable, text, text_input, tooltip,
    },
};
pub fn id() -> iced::widget::Id {
    iced::widget::Id::new("home-books")
}
fn format_tags<'a>(state: &State, labels: &'a str) -> Element<'a, Message> {
    let labels_vec: Vec<_> = labels
        .split(" · ")
        .filter(|label| !label.is_empty())
        .collect();
    let visible = if labels_vec.len() > 2 {
        1
    } else {
        labels_vec.len()
    };
    let mut tags = row![].spacing(3).align_y(Alignment::Center);
    for label in &labels_vec[..visible] {
        tags = tags.push(
            container(text(*label).size(theme::font(state, 10.0)))
                .padding([1, 4])
                .style(theme::format_tag),
        );
    }
    if labels_vec.len() > visible {
        tags = tags.push(
            text(format!("+{}", labels_vec.len() - visible))
                .size(theme::font(state, 10.0))
                .color(theme::DARK.text_muted),
        );
    }
    tooltip(
        container(tags).width(84),
        text(labels).size(theme::font(state, 12.0)),
        tooltip::Position::Top,
    )
    .into()
}
pub(super) fn ellipsis(value: &str, width: f32, size: f32) -> String {
    type TextCache = std::collections::HashMap<String, ((u32, u32), String)>;
    // Bounded per-thread layout cache: scrolling should not repeatedly shape
    // and binary-search the same title. Width/size are part of the key.
    thread_local! {
        static CACHE: std::cell::RefCell<TextCache> = std::cell::RefCell::new(TextCache::new());
    }
    let key = (width.to_bits(), size.to_bits());
    CACHE.with(|cache| {
        if let Some(result) = cache
            .borrow()
            .get(value)
            .filter(|(saved_key, _)| *saved_key == key)
            .map(|(_, value)| value)
        {
            return result.clone();
        }
        let result = measure_ellipsis(value, width, size);
        let mut cache = cache.borrow_mut();
        // Keep one width per string, and at most 2048 strings. Covers will use
        // their own LRU in item 9; this cache holds only short display strings.
        if cache.len() >= 2048 {
            cache.clear();
        }
        cache.insert(value.to_owned(), (key, result.clone()));
        result
    })
}
fn measure_ellipsis(value: &str, width: f32, size: f32) -> String {
    use iced::advanced::text::{self as advanced_text, Paragraph};
    type P = <iced::Renderer as advanced_text::Renderer>::Paragraph;
    let measure = |s: &str| {
        P::with_text(advanced_text::Text {
            content: s,
            bounds: iced::Size::new(f32::INFINITY, size * 2.0),
            size: size.into(),
            line_height: advanced_text::LineHeight::default(),
            font: iced::Font::DEFAULT,
            align_x: advanced_text::Alignment::Default,
            align_y: iced::alignment::Vertical::Center,
            shaping: advanced_text::Shaping::Advanced,
            wrapping: advanced_text::Wrapping::None,
        })
        .min_width()
    };
    if measure(value) <= width {
        return value.into();
    }
    if measure("…") > width {
        return String::new();
    }
    let boundaries: Vec<_> = value
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(value.len()))
        .collect();
    let (mut low, mut high) = (0, boundaries.len() - 1);
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = format!("{}…", &value[..boundaries[mid]]);
        if measure(&candidate) <= width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    format!("{}…", &value[..boundaries[low]])
}
pub fn view(state: &State) -> Element<'_, Message> {
    let search = state.screen == Screen::Search;
    let grid_preview = state.visible_settings().view == crate::settings::ViewMode::Grid && !search;
    let title = match &state.screen {
        Screen::Home {
            filter: Filter::Format(f),
        } => format!("Library · {}", f.label()),
        Screen::Home {
            filter: Filter::Author(a),
        } => format!("Library · {a}"),
        _ => "Home".into(),
    };
    let mut heading = row![
        text(title.clone())
            .size(theme::font(state, 16.0))
            .width(Fill),
        text(format!("{} books", state.visible_order().len()))
            .size(theme::font(state, 12.0))
            .color(theme::DARK.text_muted),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    if !state.scan_warnings.is_empty() {
        heading = heading.push(
            button(
                text(format!("{} warnings", state.scan_warnings.len()))
                    .size(theme::font(state, 12.0)),
            )
            .style(theme::quiet)
            .padding([3, 6])
            .on_press(Message::ShowWarnings),
        );
    }
    heading = heading
        .push(
            button(icons::view(icons::REFRESH))
                .style(theme::quiet)
                .padding([3, 6])
                .on_press_maybe(state.active_scan.is_none().then_some(Message::Rescan)),
        )
        .push(
            button(icons::view(icons::LIST))
                .style(move |_, s| {
                    theme::button_style(
                        if !grid_preview {
                            theme::Button::Selected
                        } else {
                            theme::Button::Quiet
                        },
                        s,
                    )
                })
                .padding([3, 6])
                .on_press(Message::ShowList),
        )
        .push(
            button(icons::view(icons::GRID))
                .style(move |_, s| {
                    theme::button_style(
                        if grid_preview {
                            theme::Button::Selected
                        } else {
                            theme::Button::Quiet
                        },
                        s,
                    )
                })
                .padding([3, 6])
                .on_press(Message::ShowGrid),
        );
    let header: Element<'_, Message> = if search {
        container(
            column![
                text_input("Search by title or author", &state.search_query)
                    .id(crate::app::search_id())
                    .on_input(Message::SearchChanged)
                    .padding([6, 10])
                    .size(theme::font(state, 13.0))
                    .style(theme::input),
                text(format!("{} results", state.search_order.len()))
                    .size(theme::font(state, 12.0))
                    .color(theme::DARK.text_muted)
            ]
            .spacing(6),
        )
        .padding([6, 12])
        .into()
    } else if state.viewport_width > 0.0 && state.viewport_width < 360.0 {
        let mut controls = row![
            text(format!("{} books", state.visible_order().len()))
                .size(theme::font(state, 12.0))
                .width(Fill)
                .color(theme::DARK.text_secondary),
            button(icons::view(icons::REFRESH))
                .style(theme::quiet)
                .on_press_maybe(state.active_scan.is_none().then_some(Message::Rescan)),
            button(icons::view(icons::LIST))
                .style(theme::quiet)
                .on_press(Message::ShowList),
            button(icons::view(icons::GRID))
                .style(theme::quiet)
                .on_press(Message::ShowGrid),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        if !state.scan_warnings.is_empty() {
            controls = controls.push(
                button(text("!").size(theme::font(state, 12.0)))
                    .style(theme::quiet)
                    .on_press(Message::ShowWarnings),
            );
        }
        container(
            column![
                text(title).size(theme::font(state, 16.0)).width(Fill),
                controls
            ]
            .spacing(4),
        )
        .padding([2, 12])
        .into()
    } else {
        container(heading)
            .padding([2, 12])
            .height(state.row_height())
            .into()
    };
    let content = column![header].height(Fill);
    let body: Element<'_, Message> = if search
        && (state.scan_status == ScanStatus::Ready || state.search_query.trim().is_empty())
        && (state.search_query.trim().is_empty()
            || state.search_running.is_some()
            || state.search_error.is_some()
            || state.search_order.is_empty())
    {
        let label = if state.search_query.trim().is_empty() {
            "Type to search by title or author"
        } else if let Some(error) = &state.search_error {
            error
        } else if state.search_running.is_some() {
            "Searching…"
        } else {
            "No results. Try another title or author."
        };
        container(
            text(label)
                .size(theme::font(state, 13.0))
                .color(theme::DARK.text_muted),
        )
        .padding(12)
        .center_x(Fill)
        .center_y(Fill)
        .into()
    } else if state.show_warnings {
        let mut warnings = column![row![
            text("Scan warnings").width(Fill),
            button("Close")
                .style(theme::quiet)
                .on_press(Message::ShowWarnings)
        ]]
        .spacing(12)
        .padding(12);
        for warning in &state.scan_warnings {
            warnings = warnings.push(
                text(warning)
                    .size(theme::font(state, 12.0))
                    .color(theme::DARK.text_secondary),
            );
        }
        scrollable(warnings)
            .style(theme::scroll)
            .height(Fill)
            .into()
    } else if state.scan_status != ScanStatus::Ready {
        let label = match &state.scan_status {
            ScanStatus::Running => "Scanning library…",
            ScanStatus::Failed(e) => e,
            ScanStatus::Cancelled => "Scan cancelled.",
            _ => "Scan the library to load books",
        };
        let mut body = column![text(label).size(theme::font(state, 13.0))].spacing(12);
        if let Some(id) = state.active_scan {
            body = body.push(
                button("Cancel scan")
                    .style(theme::outline)
                    .on_press(Message::Cancel(id)),
            );
        } else {
            body = body.push(
                button("Retry scan")
                    .style(theme::outline)
                    .on_press(Message::Rescan),
            );
            body = body.push(
                button("Change library")
                    .style(theme::outline)
                    .on_press(Message::ChangeLibrary),
            );
        }
        container(body)
            .padding(12)
            .center_x(Fill)
            .center_y(Fill)
            .into()
    } else if state.catalog.entries.is_empty() {
        container(
            column![
                text("No books found"),
                text("Rescan this library or choose another folder.")
                    .size(theme::font(state, 12.0))
                    .color(theme::DARK.text_secondary),
                row![
                    button("Rescan")
                        .style(theme::outline)
                        .on_press(Message::Rescan),
                    button("Change library")
                        .style(theme::outline)
                        .on_press(Message::ChangeLibrary)
                ]
                .spacing(8)
            ]
            .spacing(12),
        )
        .padding(20)
        .center_x(Fill)
        .center_y(Fill)
        .into()
    } else if state.visible_order().is_empty() {
        container(
            text("No books match this filter. Choose another format or author.")
                .size(theme::font(state, 13.0))
                .color(theme::DARK.text_muted),
        )
        .padding(12)
        .center_x(Fill)
        .center_y(Fill)
        .into()
    } else if grid_preview {
        super::book_grid::view(state)
    } else {
        responsive(move |size| {
            let order = state.visible_order();
            let slice = visible_slice_with_height(
                order.len(),
                state.list_offset,
                size.height,
                state.row_height(),
            );
            let mut rows =
                column![Space::new().height(slice.start as f32 * state.row_height())].spacing(0);
            let available = (size.width - 26.0 - 84.0 - 24.0 - 24.0 - 12.0).max(0.0);
            let title_width = available * 0.6;
            let author_width = available * 0.4;
            for id in &order[slice.clone()] {
                let Some(entry) = state.catalog.get(id) else {
                    continue;
                };
                let selected = state.selection.selected.contains(&entry.book.id);
                let cell = row![
                    super::book_grid::cover(state, id, &entry.book.title, 22.0, 30.0),
                    text(ellipsis(
                        &entry.book.title,
                        title_width,
                        theme::font(state, 13.0)
                    ))
                    .size(theme::font(state, 13.0))
                    .shaping(iced::widget::text::Shaping::Advanced)
                    .width(Length::Fixed(title_width)),
                    text(ellipsis(
                        &entry.book.author,
                        author_width,
                        theme::font(state, 13.0)
                    ))
                    .size(theme::font(state, 13.0))
                    .shaping(iced::widget::text::Shaping::Advanced)
                    .color(theme::DARK.text_secondary)
                    .width(Length::Fixed(author_width)),
                    format_tags(state, &entry.formats),
                ]
                .spacing(8)
                .align_y(Alignment::Center);
                rows = rows.push(
                    button(cell)
                        .height(state.row_height())
                        .width(Fill)
                        .padding([0, 12])
                        .style(move |_, status| theme::book_row(selected, status))
                        .on_press(Message::BookClicked(entry.book.id.clone())),
                );
            }
            rows = rows
                .push(Space::new().height((order.len() - slice.end) as f32 * state.row_height()));
            scrollable(rows)
                .id(id())
                .height(Fill)
                .style(theme::scroll)
                .on_scroll(|v| Message::ListScrolled {
                    offset: v.absolute_offset().y,
                    height: v.bounds().height,
                })
                .into()
        })
        .into()
    };
    let body = iced::widget::sensor(body)
        .on_show(Message::Viewport)
        .on_resize(Message::Viewport);
    content.push(body).into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_ellipsis_uses_real_width() {
        assert_eq!(ellipsis("Кириллица Łódź", 1000.0, 13.0), "Кириллица Łódź");
        let s = ellipsis("Кириллица Łódź", 40.0, 13.0);
        assert!(s.ends_with('…'));
        assert!(s.len() < "Кириллица Łódź".len());
        assert_eq!(ellipsis("wide", 0.0, 13.0), "");
    }
}
