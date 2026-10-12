//! Dark tokens from docs/design/mockup.html. Window surfaces remain opaque (D9).
use iced::{
    Border, Color, Theme,
    widget::{button, checkbox, container, scrollable, text_input},
};

pub const SIDEBAR_WIDTH: f32 = 148.0;
pub const DETAILS_WIDTH: f32 = 300.0;
pub const DETAILS_GRIP: f32 = 7.0;
pub const TOOLBAR_HEIGHT: f32 = 34.0;
pub const BOTTOM_HEIGHT: f32 = 46.0;
pub const LINE_WIDTH: f32 = 0.5;
pub const SPACE: f32 = 8.0;
pub const CONTENT_PADDING: f32 = 12.0;
pub const RADIUS: f32 = 6.0;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: Color,
    pub panel: Color,
    pub line: Color,
    pub hover: Color,
    pub selection: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub ok: Color,
    pub warning: Color,
    pub error: Color,
}
pub const DARK: Palette = Palette {
    background: Color::from_rgb8(23, 24, 27),
    panel: Color::from_rgb8(30, 32, 36),
    line: Color::from_rgba(1.0, 1.0, 1.0, 0.09),
    hover: Color::from_rgba(1.0, 1.0, 1.0, 0.07),
    selection: Color::from_rgba(91.0 / 255.0, 155.0 / 255.0, 1.0, 0.22),
    text_primary: Color::from_rgb8(232, 233, 236),
    text_secondary: Color::from_rgb8(160, 163, 173),
    text_muted: Color::from_rgb8(112, 115, 128),
    accent: Color::from_rgb8(91, 155, 255),
    ok: Color::from_rgb8(76, 195, 138),
    warning: Color::from_rgb8(229, 169, 61),
    error: Color::from_rgb8(239, 119, 119),
};
pub fn dark() -> Theme {
    Theme::custom(
        "Rulibre dark",
        iced::theme::Palette {
            background: DARK.background,
            text: DARK.text_primary,
            primary: DARK.accent,
            success: DARK.ok,
            warning: DARK.warning,
            danger: DARK.error,
        },
    )
}
pub fn surface(_: &Theme) -> container::Style {
    container::Style {
        background: Some(DARK.background.into()),
        text_color: Some(DARK.text_primary),
        ..Default::default()
    }
}
/// `focused` is the keyboard cursor: a visible outline that does not depend on colour alone.
pub fn book_row(selected: bool, focused: bool, status: button::Status) -> button::Style {
    button::Style {
        border: if focused {
            Border {
                color: DARK.accent,
                width: 1.5,
                radius: RADIUS.into(),
            }
        } else {
            Border::default()
        },
        background: if selected {
            Some(DARK.selection.into())
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            Some(DARK.hover.into())
        } else {
            None
        },
        text_color: DARK.text_primary,
        ..Default::default()
    }
}
pub fn format_tag(_: &Theme) -> container::Style {
    container::Style {
        background: Some(DARK.hover.into()),
        text_color: Some(DARK.text_secondary),
        border: Border {
            radius: 3.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
pub fn panel(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(DARK.panel.into()),
        ..surface(theme)
    }
}
pub fn line(_: &Theme) -> container::Style {
    container::Style {
        background: Some(DARK.line.into()),
        ..Default::default()
    }
}
pub fn card(theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: DARK.line,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..panel(theme)
    }
}
#[derive(Clone, Copy)]
pub enum Button {
    Outline,
    Primary,
    Quiet,
    Selected,
}
pub fn button_style(kind: Button, status: button::Status) -> button::Style {
    let disabled = status == button::Status::Disabled;
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let background = match kind {
        Button::Primary => Some(
            DARK.accent
                .scale_alpha(if disabled { 0.3 } else { 1.0 })
                .into(),
        ),
        Button::Selected => Some(DARK.hover.into()),
        _ if hovered => Some(DARK.hover.into()),
        _ => None,
    };
    button::Style {
        background,
        text_color: if disabled {
            DARK.text_muted
        } else if matches!(kind, Button::Quiet) {
            DARK.text_secondary
        } else {
            DARK.text_primary
        },
        border: Border {
            color: DARK.line,
            width: if matches!(kind, Button::Outline) {
                LINE_WIDTH
            } else {
                0.0
            },
            radius: RADIUS.into(),
        },
        ..Default::default()
    }
}
pub fn outline(_: &Theme, status: button::Status) -> button::Style {
    button_style(Button::Outline, status)
}
pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    button_style(Button::Primary, status)
}
pub fn quiet(_: &Theme, status: button::Status) -> button::Style {
    button_style(Button::Quiet, status)
}
pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: DARK.background.into(),
        border: Border {
            color: if matches!(status, text_input::Status::Focused { .. }) {
                DARK.accent
            } else {
                DARK.line
            },
            width: 1.0,
            radius: RADIUS.into(),
        },
        icon: DARK.text_secondary,
        placeholder: DARK.text_muted,
        value: if status == text_input::Status::Disabled {
            DARK.text_muted
        } else {
            DARK.text_primary
        },
        selection: DARK.selection,
    }
}
/// Seconds of inactivity after which scrollbars fade out.
pub const SCROLLBAR_IDLE_SECS: u64 = 5;
/// Slim overlay scrollbar (no layout space, rounded 4 px scroller).
pub fn vbar() -> scrollable::Direction {
    scrollable::Direction::Vertical(
        scrollable::Scrollbar::new()
            .width(4)
            .scroller_width(4)
            .margin(2),
    )
}
/// Scrollbars are visible while the user scrolls, drags or hovers one, and for
/// `SCROLLBAR_IDLE_SECS` afterwards; otherwise the scroller is transparent.
pub fn scroll(
    state: &crate::app::State,
) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style + 'static {
    let recently_active = state.scroll_visible;
    move |theme, status| {
        let mut style = scrollable::default(theme, status);
        let (hovered, dragged) = match status {
            scrollable::Status::Hovered {
                is_vertical_scrollbar_hovered,
                ..
            } => (is_vertical_scrollbar_hovered, false),
            scrollable::Status::Dragged {
                is_vertical_scrollbar_dragged,
                ..
            } => (false, is_vertical_scrollbar_dragged),
            scrollable::Status::Active { .. } => (false, false),
        };
        let alpha = if dragged {
            0.8
        } else if hovered {
            0.65
        } else if recently_active {
            0.45
        } else {
            0.0
        };
        for rail in [&mut style.vertical_rail, &mut style.horizontal_rail] {
            rail.background = None;
            rail.scroller.background = DARK.text_muted.scale_alpha(alpha).into();
            rail.scroller.border = Border {
                radius: 2.0.into(),
                ..Default::default()
            };
        }
        style
    }
}
pub fn check(_: &Theme, status: checkbox::Status) -> checkbox::Style {
    let (checked, disabled) = match status {
        checkbox::Status::Active { is_checked } | checkbox::Status::Hovered { is_checked } => {
            (is_checked, false)
        }
        checkbox::Status::Disabled { is_checked } => (is_checked, true),
    };
    checkbox::Style {
        background: if checked {
            DARK.accent
        } else {
            DARK.background
        }
        .into(),
        icon_color: DARK.text_primary,
        border: Border {
            color: DARK.line,
            width: 1.0,
            radius: 3.0.into(),
        },
        text_color: Some(if disabled {
            DARK.text_muted
        } else {
            DARK.text_primary
        }),
    }
}

/// Font-only scaling; layout geometry stays independent of OS DPI.
pub fn font(state: &crate::app::State, base: f32) -> f32 {
    base * state.visible_settings().text_size as f32 / 13.0
}
pub fn conversion_menu(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb8(41, 45, 53).into()),
        border: Border {
            color: Color::from_rgb8(89, 97, 113),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}
pub fn conversion_route(_: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: Some(
            if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                Color::from_rgb8(65, 95, 137)
            } else {
                Color::from_rgb8(51, 68, 94)
            }
            .into(),
        ),
        text_color: Color::from_rgb8(244, 246, 255),
        border: Border {
            color: Color::from_rgb8(102, 136, 181),
            width: 1.0,
            radius: 5.0.into(),
        },
        ..Default::default()
    }
}
