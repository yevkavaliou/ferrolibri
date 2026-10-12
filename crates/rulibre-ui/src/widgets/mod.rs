pub mod book_grid;
pub mod book_list;
pub mod convert;
pub mod device_bar;
pub mod jobs;
pub mod settings;
pub mod shell;
pub mod toasts;

pub mod details;

use crate::{app::Message, theme};
use iced::Element;

pub use iced::widget::tooltip::Position;

/// A hint that appears after a short pause, on an opaque background.
pub fn tip<'a>(
    content: impl Into<Element<'a, Message>>,
    hint: impl Into<Element<'a, Message>>,
    position: Position,
) -> Element<'a, Message> {
    iced::widget::tooltip(content, hint, position)
        .delay(theme::TOOLTIP_DELAY)
        .padding(6)
        .style(theme::tooltip)
        .into()
}
