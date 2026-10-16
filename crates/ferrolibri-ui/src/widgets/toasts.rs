use crate::{
    app::{Message, State},
    icons, theme,
};
use iced::{
    Element, Fill,
    widget::{button, column, container, row, scrollable, text},
};
use std::time::{Duration, Instant};

pub const LIFETIME: Duration = Duration::from_secs(5);
#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub message: String,
    pub error: bool,
    pub expires: Option<Instant>,
    pub folder: Option<std::path::PathBuf>,
}
pub fn push(state: &mut State, message: String, error: bool) {
    state.next_toast += 1;
    state.toasts.push(Toast {
        id: state.next_toast,
        message,
        error,
        // Start on the next clock event, including when the clock subscription
        // had stopped while idle. Never reuse an old event-loop timestamp.
        expires: None,
        folder: None,
    });
}
pub fn advance(state: &mut State, now: Instant) {
    for toast in &mut state.toasts {
        if !toast.error && toast.expires.is_none() {
            toast.expires = Some(now + LIFETIME);
        }
    }
    state
        .toasts
        .retain(|t| t.expires.is_none_or(|expiry| now < expiry));
}
pub fn view(state: &State) -> Element<'_, Message> {
    let mut items = column![].spacing(8);
    for toast in &state.toasts {
        let mut content = column![card(
            state,
            &toast.message,
            toast.error,
            Message::DismissToast(toast.id)
        )];
        if let Some(folder) = &toast.folder {
            content = content.push(
                button(text("Open folder").size(theme::font(state, 12.0)))
                    .style(theme::quiet)
                    .on_press(Message::OpenFolder(folder.clone())),
            );
        }
        items = items.push(content);
    }
    for (index, notice) in state.notices.iter().enumerate() {
        items = items.push(card(state, notice, true, Message::DismissNotice(index)));
    }
    container(
        scrollable(items)
            .direction(theme::vbar())
            .style(theme::scroll(state)),
    )
    .width(340)
    .max_height(280)
    .into()
}
fn card<'a>(state: &State, message: &'a str, error: bool, close: Message) -> Element<'a, Message> {
    container(
        row![
            text(if error { "!" } else { "✓" }).color(if error {
                theme::DARK.error
            } else {
                theme::DARK.ok
            }),
            text(message).size(theme::font(state, 12.0)).width(Fill),
            button(icons::view(icons::X))
                .padding(2)
                .style(theme::quiet)
                .on_press(close),
        ]
        .spacing(8),
    )
    .padding(12)
    .style(theme::card)
    .into()
}
