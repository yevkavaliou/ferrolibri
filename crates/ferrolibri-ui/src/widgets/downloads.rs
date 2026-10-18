//! The download icon, the "Download as" menu and the "Calibre is needed" card (design gap G6),
//! drawn after `docs/design/mockup.html`: the same states, tooltips and words. The state itself
//! is `crate::downloads`; this module only draws it.
use crate::{
    app::{Message, State},
    downloads::{self, Icon, Msg},
    icons,
    services::plugins::{RemoteHit, RemoteOffer},
    theme,
};
use iced::{
    Alignment, Color, Element, Fill, Length, Radians, Rotation,
    widget::{button, column, container, row, svg, text},
};

/// Width of the "Download as" menu.
pub const MENU_WIDTH: f32 = 240.0;
/// The room an icon in a row or on a card takes.
pub const ICON_SIZE: f32 = 30.0;

fn message(hit: &RemoteHit, offer: Option<&RemoteOffer>) -> Message {
    Message::Download(Msg::Press {
        hit: hit.id.clone(),
        offer: offer.map(|o| o.id.clone()),
    })
}

/// The colour of the glyph: the mockup's plain, "off", "busy", "ok" and "bad" classes.
fn tint(icon: Icon) -> Color {
    match icon {
        Icon::Idle | Icon::Queued => theme::DARK.text_secondary,
        Icon::NeedsCalibre | Icon::NoFile => theme::DARK.text_muted.scale_alpha(0.7),
        Icon::Downloading(_) | Icon::Adding => theme::DARK.accent,
        Icon::Done | Icon::Exists => theme::DARK.ok,
        Icon::Failed => theme::DARK.error,
    }
}

/// A ring that fills as bytes arrive, or a quarter-ring that turns when nothing is known.
fn ring<'a>(percent: Option<u8>, spin: u32, color: Color) -> Element<'a, Message> {
    // 2 * pi * 8: the ring has a radius of 8 in a 24 by 24 drawing.
    const CIRCUMFERENCE: f32 = 50.265;
    let (arc, turn) = match percent {
        Some(percent) => (CIRCUMFERENCE * f32::from(percent.min(100)) / 100.0, 0.0),
        None => (
            CIRCUMFERENCE * 0.28,
            spin as f32 * std::f32::consts::FRAC_PI_6,
        ),
    };
    let data = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="3"><circle cx="12" cy="12" r="8" stroke-opacity="0.25"/><circle cx="12" cy="12" r="8" stroke-dasharray="{arc:.2} {gap:.2}" transform="rotate(-90 12 12)"/></svg>"#,
        gap = CIRCUMFERENCE - arc + 0.01,
    );
    svg(svg::Handle::from_memory(data.into_bytes()))
        .width(16)
        .height(16)
        .rotation(Rotation::Floating(Radians(turn)))
        .style(move |_, _| svg::Style { color: Some(color) })
        .into()
}

fn glyph<'a>(state: &State, icon: Icon) -> Element<'a, Message> {
    let color = tint(icon);
    match icon {
        Icon::Idle | Icon::NeedsCalibre => icons::tinted(icons::DOWNLOAD, color).into(),
        Icon::NoFile => icons::tinted(icons::DOWNLOAD_OFF, color).into(),
        Icon::Queued => icons::tinted(icons::CLOCK, color).into(),
        Icon::Downloading(percent) => ring(percent, state.downloads.spin(), color),
        Icon::Adding => ring(None, state.downloads.spin(), color),
        Icon::Done => icons::tinted(icons::CHECK, color).into(),
        Icon::Exists => icons::tinted(icons::BOOKS, color).into(),
        Icon::Failed => icons::tinted(icons::ALERT, color).into(),
    }
}

/// The icon of a result, in a list row or over a cover. A result with one offer downloads on a
/// click; with several, the click opens the format menu.
pub fn icon_button<'a>(state: &State, hit: &RemoteHit, over_cover: bool) -> Element<'a, Message> {
    let icon = downloads::hit_icon(state, hit);
    let control = button(container(glyph(state, icon)).center_x(Fill).center_y(Fill))
        .width(ICON_SIZE)
        .height(ICON_SIZE)
        .padding(0)
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let mut style = theme::button_style(theme::Button::Quiet, status);
            if over_cover {
                // Light glyphs on a dark disc stay readable on any cover.
                style.background = Some(
                    Color::BLACK
                        .scale_alpha(if hovered { 0.8 } else { 0.65 })
                        .into(),
                );
                style.border.radius = (ICON_SIZE / 2.0).into();
            }
            style
        })
        .on_press_maybe((icon != Icon::NoFile).then(|| message(hit, None)));
    super::tip(
        control,
        text(icon.tip()).size(theme::font(state, 12.0)),
        super::Position::Bottom,
    )
}

/// The labelled button of one offer in the details pane.
pub fn offer_button<'a>(
    state: &State,
    hit: &RemoteHit,
    offer: &RemoteOffer,
) -> Element<'a, Message> {
    let icon = downloads::offer_icon(state, hit, offer);
    let enabled = downloads::can_start(state, hit, offer) || icon == Icon::NeedsCalibre;
    let control = button(
        row![
            glyph(state, icon),
            text(icon.label()).size(theme::font(state, 12.0))
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([3, 10])
    .style(theme::outline)
    .on_press_maybe(enabled.then(|| message(hit, Some(offer))));
    super::tip(
        control,
        text(icon.tip()).size(theme::font(state, 12.0)),
        super::Position::Bottom,
    )
}

/// "DOWNLOAD AS": one line per offer with its own state. The shell puts it under the pointer.
pub fn menu(state: &State) -> Option<Element<'_, Message>> {
    let open = state.downloads.menu.as_ref()?;
    let hit = state.remote.hit(&open.hit)?;
    let mut items = column![
        container(
            text("DOWNLOAD AS")
                .size(theme::font(state, 11.0))
                .color(Color::from_rgb8(204, 215, 230))
        )
        .padding([6, 8])
    ]
    .width(Fill);
    for offer in &hit.offers {
        let icon = downloads::offer_icon(state, hit, offer);
        let what = match &offer.label {
            Some(label) => format!("{label} · {}", super::remote::size_text(offer.size)),
            None => super::remote::size_text(offer.size),
        };
        let line = row![
            container(text(downloads::offer_label(offer)).size(theme::font(state, 10.0)))
                .padding([1, 4])
                .style(theme::format_tag),
            text(what)
                .size(theme::font(state, 12.0))
                .color(Color::from_rgb8(204, 215, 230))
                .width(Fill),
            glyph(state, icon),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        items = items.push(
            button(line)
                .width(Fill)
                .padding([7, 8])
                .style(|_, status| {
                    let mut style = theme::button_style(theme::Button::Quiet, status);
                    style.text_color = Color::from_rgb8(244, 246, 255);
                    if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                        style.background = Some(Color::from_rgb8(65, 95, 137).into());
                    }
                    style
                })
                .on_press_maybe(
                    downloads::can_start(state, hit, offer).then(|| message(hit, Some(offer))),
                ),
        );
    }
    Some(
        container(items)
            .width(MENU_WIDTH)
            .padding(4)
            .style(theme::conversion_menu)
            .into(),
    )
}

/// Height of the menu for `offers` lines, to keep it inside the window.
pub fn menu_height(offers: usize) -> f32 {
    8.0 + 28.0 + offers as f32 * 34.0
}

/// The card that replaces the guesswork when `calibredb` is missing: what to install, and
/// "Check again". Nothing when Calibre is there (or not looked for yet).
pub fn calibre_card<'a>(state: &State) -> Option<Element<'a, Message>> {
    if state.downloads.calibre != Some(false) {
        return None;
    }
    let small = theme::font(state, 12.0);
    let body = column![
        text(downloads::NEEDS_CALIBRE)
            .size(theme::font(state, 13.0))
            .font(iced::Font {
                weight: iced::font::Weight::Bold,
                ..iced::Font::DEFAULT
            }),
        text(
            "FerroLibri adds books with Calibre’s calibredb tool, which was not found. \
             Install Calibre, then check again:"
        )
        .size(small)
        .color(theme::DARK.text_secondary),
        container(
            text("winget install --id calibre.calibre")
                .size(small)
                .font(iced::Font::MONOSPACE)
        )
        .padding(6)
        .width(Fill)
        .style(theme::card),
        text("macOS: brew install --cask calibre · Linux and others: calibre-ebook.com/download")
            .size(small)
            .color(theme::DARK.text_secondary),
        button(text("Check again").size(small))
            .style(theme::outline)
            .padding([3, 9])
            .on_press(Message::Download(Msg::CheckCalibre)),
    ]
    .spacing(6);
    Some(
        container(body)
            .padding(12)
            .width(Fill)
            .style(|theme| {
                let mut style = theme::card(theme);
                style.background = Some(theme::DARK.background.into());
                style.border.radius = 6.0.into();
                style
            })
            .into(),
    )
}

/// The icon of a list row, right-aligned over the end of the row.
pub fn row_overlay<'a>(state: &State, hit: &RemoteHit, row_height: f32) -> Element<'a, Message> {
    container(icon_button(state, hit, false))
        .width(Fill)
        .height(Length::Fixed(row_height))
        .align_right(Fill)
        .center_y(Length::Fixed(row_height))
        .padding([0, 8])
        .into()
}

/// The icon of a grid card, at the bottom right of its cover.
pub fn cover_overlay<'a>(
    state: &State,
    hit: &RemoteHit,
    card_width: f32,
    cover_width: f32,
    cover_height: f32,
) -> Element<'a, Message> {
    // The card has 6 of padding and centres the cover in its width.
    let left =
        6.0 + (card_width - 12.0 - cover_width).max(0.0) / 2.0 + cover_width - ICON_SIZE - 4.0;
    let top = 6.0 + cover_height - ICON_SIZE - 4.0;
    container(icon_button(state, hit, true))
        .width(Fill)
        .height(Fill)
        .padding(iced::Padding {
            top: top.max(0.0),
            left: left.max(0.0),
            ..Default::default()
        })
        .into()
}
