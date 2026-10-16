use crate::settings::{COVER_SIZE_RANGE, COVER_SIZE_STEP, TEXT_SIZE_RANGE};
use crate::{
    app::{Message, State},
    theme,
};
use iced::{
    Element, Fill,
    widget::{button, column, container, row, scrollable, slider, text, toggler},
};

/// `−  16  +` for the interface text size.
fn stepper(state: &State, value: u16) -> iced::widget::Row<'_, Message> {
    let step = |label: &'static str, target: u16, enabled: bool| {
        button(
            text(label)
                .size(theme::font(state, 14.0))
                .width(Fill)
                .align_x(iced::alignment::Horizontal::Center),
        )
        .width(30)
        .padding([2, 0])
        .style(theme::outline)
        .on_press_maybe((enabled && !state.closing).then_some(Message::SetTextSize(target)))
    };
    row![
        step("−", value.saturating_sub(1), value > TEXT_SIZE_RANGE.0),
        text(value.to_string())
            .size(theme::font(state, 13.0))
            .width(36)
            .align_x(iced::alignment::Horizontal::Center),
        step("+", value + 1, value < TEXT_SIZE_RANGE.1),
    ]
    .spacing(6)
    .align_y(iced::Center)
}

pub fn view(state: &State) -> Element<'_, Message> {
    let settings = state.visible_settings();
    let available = crate::blur::ready() && !state.blur.failed;
    let library = state
        .library
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Not selected".into());
    let export = settings
        .export_folder
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Not selected".into());
    let busy = state.setup_busy || state.export_busy || state.library_loading || state.closing;
    let mut library_button =
        button(text("Change…").size(theme::font(state, 12.0))).style(theme::quiet);
    let mut export_button =
        button(text("Change…").size(theme::font(state, 12.0))).style(theme::quiet);
    if !busy {
        library_button = library_button.on_press(Message::SettingsLibraryBrowse);
        if state.library.is_some() {
            export_button = export_button.on_press(Message::ExportBrowse);
        }
    }
    let mut kepub = toggler(settings.prefer_kepub).size(20);
    if !state.closing {
        kepub = kepub.on_toggle(Message::SetPreferKepub);
    }
    let rows = column![
        setting_row(
            state,
            "Text size",
            "Pixels · applies throughout the interface",
            stepper(state, settings.text_size).into()
        ),
        setting_row(
            state,
            "Cover size",
            "Width in pixels · grid and book details",
            row![
                slider(
                    COVER_SIZE_RANGE.0..=COVER_SIZE_RANGE.1,
                    settings.cover_size,
                    Message::SetCoverSize,
                )
                .step(COVER_SIZE_STEP)
                .width(160),
                text(format!("{} px", settings.cover_size))
                    .size(theme::font(state, 13.0))
                    .width(52)
                    .align_x(iced::alignment::Horizontal::Right),
            ]
            .spacing(10)
            .align_y(iced::Center)
            .into()
        ),
        setting_row(
            state,
            "Window opacity",
            if available {
                "Lower values let the desktop show through the window · 100 % is opaque"
            } else {
                "Needs Windows 10 or 11 with DirectX 12 · not available here"
            },
            row![
                {
                    let mut control = slider(
                        crate::blur::MIN_OPACITY..=100u8,
                        settings.window_opacity,
                        Message::SetWindowOpacity,
                    )
                    .step(1u8)
                    .width(160);
                    if !available {
                        // A slider without a handler cannot be dragged; keep it inert.
                        control =
                            slider(crate::blur::MIN_OPACITY..=100u8, 100, |_| Message::Ignore)
                                .width(160);
                    }
                    control
                },
                text(format!("{} %", settings.window_opacity))
                    .size(theme::font(state, 13.0))
                    .width(52)
                    .align_x(iced::alignment::Horizontal::Right),
            ]
            .spacing(10)
            .align_y(iced::Center)
            .into()
        ),
        setting_row(
            state,
            "Blur window",
            "Draw the window over a blurred desktop · needs an opacity below 100 %",
            {
                let mut blur = toggler(settings.window_blur && available).size(20);
                if available && !state.closing {
                    blur = blur.on_toggle(Message::SetWindowBlur);
                }
                blur.into()
            }
        ),
        setting_row(
            state,
            "Prefer KEPUB for Kobo",
            "Convert EPUB to KEPUB before sending",
            kepub.into()
        ),
        setting_row(state, "Library folder", library, library_button.into()),
        setting_row(state, "Export folder", export, export_button.into()),
    ]
    .spacing(24);
    column![
        container(text("Settings").size(theme::font(state, 16.0)))
            .padding(super::book_list::header_padding())
            .height(36.0 + theme::HEADER_TOP),
        scrollable(container(rows).padding(20).width(Fill))
            .direction(theme::vbar())
            .style(theme::scroll(state)),
    ]
    .height(Fill)
    .into()
}
fn setting_row<'a>(
    state: &State,
    label: &'a str,
    hint: impl Into<String>,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    row![
        column![
            text(label).size(theme::font(state, 14.0)).width(Fill),
            text(hint.into())
                .size(theme::font(state, 12.0))
                .width(Fill)
                .color(theme::DARK.text_secondary)
        ]
        .spacing(4)
        .width(Fill),
        control,
    ]
    .spacing(16)
    .align_y(iced::Center)
    .into()
}
