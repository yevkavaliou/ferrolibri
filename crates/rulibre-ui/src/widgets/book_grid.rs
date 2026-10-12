//! Fixed-height virtual cover grid. Layout and loading share the same geometry.
use crate::{
    app::{Message, State},
    theme,
};
use iced::{
    Alignment, Element, Fill,
    widget::{Space, button, column, container, image, responsive, row, scrollable, text},
};
use std::ops::Range;
pub const GAP: f32 = 12.0;
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub cover_width: f32,
    pub cover_height: f32,
    pub card_width: f32,
    pub row_height: f32,
}
impl Geometry {
    pub fn for_state(state: &State) -> Self {
        let width = state.visible_settings().cover_size as f32;
        Self {
            cover_width: width,
            cover_height: width * 1.375,
            card_width: width + 12.0,
            row_height: width * 1.375 + theme::font(state, 32.0) + 24.0,
        }
    }
    pub fn columns(self, width: f32) -> usize {
        (((width - 24.0 + GAP) / (self.card_width + GAP)).floor() as usize).max(1)
    }
    pub fn book_range(self, count: usize, width: f32, offset: f32, height: f32) -> Range<usize> {
        let cols = self.columns(width);
        let rows = count.div_ceil(cols);
        let start = ((offset.max(0.0) / self.row_height).floor() as usize)
            .saturating_sub(1)
            .min(rows);
        let end =
            (((offset.max(0.0) + height.max(0.0)) / self.row_height).ceil() as usize + 1).min(rows);
        (start * cols).min(count)..(end * cols).min(count)
    }
}
pub fn placeholder_color(title: &str) -> iced::Color {
    let hash = title.as_bytes().iter().fold(2166136261u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(16777619)
    });
    iced::Color::from_rgb8(
        45 + (hash % 65) as u8,
        45 + ((hash >> 8) % 65) as u8,
        60 + ((hash >> 16) % 65) as u8,
    )
}
pub fn cover<'a>(
    state: &'a State,
    id: &rulibre_core::book::BookId,
    title: &'a str,
    width: f32,
    height: f32,
) -> Element<'a, Message> {
    if let Some(handle) = state.covers.get(id) {
        container(
            image(handle.clone())
                .content_fit(iced::ContentFit::Contain)
                .width(width)
                .height(height),
        )
        .width(width)
        .height(height)
        .into()
    } else {
        let color = placeholder_color(title);
        container(
            text(super::book_list::ellipsis(
                title,
                (width - 10.0).max(0.0),
                theme::font(state, 10.0),
            ))
            .size(theme::font(state, 10.0)),
        )
        .padding(5)
        .width(width)
        .height(height)
        .align_y(iced::alignment::Vertical::Bottom)
        .style(move |_| iced::widget::container::Style {
            background: Some(color.into()),
            text_color: Some(iced::Color::WHITE),
            border: iced::Border::default().rounded(5),
            ..Default::default()
        })
        .into()
    }
}
pub fn view(state: &State) -> Element<'_, Message> {
    responsive(move |size| {
        let order = state.visible_order();
        let geometry = Geometry::for_state(state);
        let cols = geometry.columns(size.width);
        let range = geometry.book_range(order.len(), size.width, state.list_offset, size.height);
        let mut rows =
            column![Space::new().height(range.start.div_ceil(cols) as f32 * geometry.row_height)]
                .spacing(0);
        for chunk in order[range.clone()].chunks(cols) {
            let mut cards = row![].spacing(GAP);
            for id in chunk {
                let Some(entry) = state.catalog.get(id) else {
                    continue;
                };
                let selected = state.selection.selected.contains(id);
                let card = column![
                    cover(
                        state,
                        id,
                        &entry.book.title,
                        geometry.cover_width,
                        geometry.cover_height
                    ),
                    text(super::book_list::ellipsis(
                        &entry.book.title,
                        geometry.cover_width,
                        theme::font(state, 13.0)
                    ))
                    .size(theme::font(state, 13.0)),
                    text(super::book_list::ellipsis(
                        &entry.book.author,
                        geometry.cover_width,
                        theme::font(state, 11.0)
                    ))
                    .size(theme::font(state, 11.0))
                    .color(theme::DARK.text_muted),
                ]
                .spacing(3)
                .align_x(Alignment::Center);
                cards = cards.push(
                    button(card)
                        .width(geometry.card_width)
                        .padding(6)
                        .style(move |_, status| theme::book_row(selected, status))
                        .on_press(Message::BookClicked(id.clone())),
                );
            }
            rows = rows.push(
                container(cards)
                    .height(geometry.row_height)
                    .padding([0, 12]),
            );
        }
        rows = rows.push(Space::new().height(
            (order.len().div_ceil(cols) - range.end.div_ceil(cols)) as f32 * geometry.row_height,
        ));
        scrollable(rows)
            .id(super::book_list::id())
            .height(Fill)
            .style(theme::scroll)
            .on_scroll(|v| Message::ListScrolled {
                offset: v.absolute_offset().y,
                height: v.bounds().height,
            })
            .into()
    })
    .into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_geometry_and_placeholders() {
        let geometry = Geometry::for_state(&State::default());
        assert_eq!(geometry.columns(0.0), 1);
        assert_eq!(geometry.columns(240.0), 1);
        assert_eq!(geometry.columns(252.0), 1);
        assert_eq!(geometry.columns(732.0), 3);
        assert_eq!(
            geometry.book_range(
                5000,
                732.0,
                geometry.row_height * 10.25,
                geometry.row_height * 2.0
            ),
            27..42
        );
        assert_eq!(geometry.book_range(0, 732.0, 0.0, 600.0), 0..0);
        assert_eq!(geometry.book_range(7, 252.0, 99999.0, 300.0), 7..7);
        assert_eq!(
            placeholder_color("Книга Łódź"),
            placeholder_color("Книга Łódź")
        );
        assert_ne!(placeholder_color("Book A"), placeholder_color("Book B"));
    }
}
