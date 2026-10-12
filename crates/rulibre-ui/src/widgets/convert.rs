//! Convert control: a button in the library header and the menu it opens.
use super::details::{TARGETS, selected, targets, unavailable};
use crate::{
    app::{Message, State},
    icons, theme,
};
use iced::{
    Alignment, Element, Fill,
    widget::{Space, button, column, container, row, scrollable, text},
};
use rulibre_core::converter::{self, BackendStatus};

pub const MENU_WIDTH: f32 = 300.0;

/// Header button. Idle until a book is selected.
pub fn button_view(state: &State) -> Element<'static, Message> {
    let enabled = !state.selection.selected.is_empty();
    let open = state.convert_menu && enabled;
    let color = if enabled {
        theme::DARK.text_secondary
    } else {
        theme::DARK.text_muted.scale_alpha(0.5)
    };
    super::tip(
        button(
            row![
                icons::tinted(icons::CONVERT, color),
                text("Convert").size(theme::font(state, 12.0)),
                icons::tinted(icons::CHEVRON_DOWN, color),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([3, 8])
        .style(move |_, status| {
            theme::button_style(
                if open {
                    theme::Button::Selected
                } else {
                    theme::Button::Quiet
                },
                status,
            )
        })
        .on_press_maybe(enabled.then_some(Message::ToggleConvert)),
        text(if enabled {
            crate::keymap::tip("Convert", crate::keymap::Command::ToggleConvert)
        } else {
            "Select a book to convert it".to_owned()
        })
        .size(theme::font(state, 12.0)),
        super::Position::Bottom,
    )
}

fn separator<'a>() -> Element<'a, Message> {
    container(Space::new())
        .width(Fill)
        .height(theme::LINE_WIDTH)
        .style(theme::line)
        .into()
}

/// The dropdown card; the shell places it under the header button.
pub fn menu(state: &State) -> Element<'_, Message> {
    let books = selected(state);
    let mut items = column![
        text(if books.len() > 1 {
            format!("Convert {} books to", books.len())
        } else {
            "Convert to".to_owned()
        })
        .size(theme::font(state, 11.0))
        .color(theme::DARK.text_muted),
    ]
    .spacing(2)
    .width(Fill);
    let available = targets(&books, &state.backends);
    for target in TARGETS {
        let label = target.label();
        if let Some((_, route)) = books
            .first()
            .filter(|_| available.contains(&target))
            .and_then(|book| {
                converter::target_formats(&book.files, &state.backends)
                    .into_iter()
                    .find(|(format, _)| *format == target)
            })
        {
            items = items.push(
                button(
                    row![
                        text(label)
                            .size(theme::font(state, 12.0))
                            .color(theme::DARK.text_primary)
                            .width(Fill),
                        text(route.to_string())
                            .size(theme::font(state, 11.0))
                            .color(theme::DARK.text_muted),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .width(Fill)
                .padding([5, 8])
                .style(theme::quiet)
                .on_press(Message::Convert(target)),
            );
        } else {
            let reasons = books
                .iter()
                .filter_map(|book| {
                    let reason = unavailable(book, target, &state.backends);
                    (!reason.is_empty()).then(|| format!("{}: {reason}", book.title))
                })
                .collect::<Vec<_>>()
                .join("\n");
            let row = button(
                row![
                    text(label).size(theme::font(state, 12.0)).width(Fill),
                    text("no route").size(theme::font(state, 11.0)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(Fill)
            .padding([5, 8])
            .style(theme::quiet);
            items = items.push(super::tip(
                row,
                container(text(reasons).size(theme::font(state, 11.0)))
                    .max_width(320)
                    .max_height(220),
                super::Position::Left,
            ));
        }
    }
    items = items.push(separator());
    items = items.push(
        button(
            text(if state.backend_job.is_some() {
                "Finding converters…"
            } else {
                "Refresh converters"
            })
            .size(theme::font(state, 11.0)),
        )
        .padding([4, 8])
        .style(theme::quiet)
        .on_press_maybe(
            state
                .backend_job
                .is_none()
                .then_some(Message::RefreshBackends),
        ),
    );
    if let Some(error) = &state.backend_error {
        items = items.push(
            text(error)
                .size(theme::font(state, 11.0))
                .color(theme::DARK.error),
        );
    }
    if state.backend_job.is_none()
        && !state
            .backends
            .iter()
            .any(|b| b.status == BackendStatus::Ready)
    {
        items = items.push(
            text("No converter found. Install Calibre, then choose Refresh converters.")
                .size(theme::font(state, 11.0))
                .color(theme::DARK.warning),
        );
    }
    container(
        scrollable(container(items).padding([0, 6]))
            .height(iced::Length::Shrink)
            .direction(theme::vbar())
            .style(theme::scroll(state)),
    )
    .padding([8, 2])
    .width(MENU_WIDTH)
    .max_height(420)
    .style(theme::card)
    .into()
}
