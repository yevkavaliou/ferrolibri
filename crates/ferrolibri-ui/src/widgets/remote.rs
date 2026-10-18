//! The Search screen with a plugin as its source (design gaps G4 and G5), drawn after
//! `docs/design/mockup.html`: the source switch, the states of a search, the rows and cards of
//! results, and the pane with the details of the selected one. The texts are the mockup's.
//!
//! Not here: the download icon on rows, cards and offers, and the "Calibre is needed" card.
//! They belong to item 13 (design gap G6).
use crate::{
    app::{Message, State},
    icons,
    remote::{self, DetailsPhase, More, Msg, Phase},
    services::plugins::{PluginRow, RemoteHit, RemoteOffer, SourceError, SourceErrorKind},
    theme,
};
use iced::{
    Alignment, Border, Element, Fill, Length,
    widget::{
        Space, button, column, container, image, pick_list, responsive, row, scrollable, text,
        text_input,
    },
};

fn msg(m: Msg) -> Message {
    Message::Remote(m)
}

fn small<'a>(state: &State, content: impl Into<String>) -> iced::widget::Text<'a> {
    text(content.into())
        .size(theme::font(state, 12.0))
        .color(theme::DARK.text_secondary)
}

fn muted<'a>(state: &State, content: impl Into<String>) -> iced::widget::Text<'a> {
    text(content.into())
        .size(theme::font(state, 11.0))
        .color(theme::DARK.text_muted)
}

// ---- the source switch (G4) -----------------------------------------------------------------

/// "Library" and one button per plugin, left of the search field. Up to three sources are
/// buttons side by side; more are a drop-down list. A plugin that is off or not compatible is
/// shown, inactive, with the reason in its hint.
pub fn switcher(state: &State) -> Option<Element<'_, Message>> {
    if !remote::switcher_visible(state) {
        return None;
    }
    let choices = remote::choices(state);
    let now = remote::current(state);
    if choices.len() > 3 {
        let selected = choices.iter().find(|c| c.source == now).cloned();
        return Some(
            pick_list(choices, selected, |choice: remote::Choice| {
                msg(Msg::SourceChosen(choice.source))
            })
            .text_size(theme::font(state, 12.0))
            .padding([4, 8])
            .into(),
        );
    }
    let size = theme::font(state, 12.0);
    let mut buttons = row![].align_y(Alignment::Center);
    for choice in choices {
        let on = choice.source == now;
        let label = super::book_list::ellipsis(&choice.name, 140.0, size);
        let control = button(text(label).size(size))
            .padding([3, 8])
            .style(move |_, status| {
                theme::button_style(
                    if on {
                        theme::Button::Selected
                    } else {
                        theme::Button::Quiet
                    },
                    status,
                )
            })
            .on_press_maybe(
                choice
                    .usable
                    .then(|| msg(Msg::SourceChosen(choice.source.clone()))),
            );
        buttons = buttons.push(if choice.usable {
            Element::from(control)
        } else {
            super::tip(
                control,
                text(choice.why).size(size),
                super::Position::Bottom,
            )
        });
    }
    Some(
        container(buttons)
            .style(|_| container::Style {
                border: Border {
                    color: theme::DARK.line,
                    width: 1.0,
                    radius: theme::RADIUS.into(),
                },
                ..Default::default()
            })
            .into(),
    )
}

/// What the plugin says about its search (`search_hint`), under the field.
fn hint_line(state: &State, row: &PluginRow) -> Option<Element<'static, Message>> {
    let hint = row.info.as_ref()?.search_hint.clone()?;
    Some(container(muted(state, hint)).padding([0, 2]).into())
}

fn view_switch(state: &State) -> Element<'_, Message> {
    let grid = state.remote.grid;
    let toggle = |icon: &'static [u8], on: bool, message: Msg, label: &'static str| {
        super::tip(
            button(icons::view(icon))
                .style(move |_, status| {
                    theme::button_style(
                        if on {
                            theme::Button::Selected
                        } else {
                            theme::Button::Quiet
                        },
                        status,
                    )
                })
                .padding([3, 6])
                .on_press(msg(message)),
            text(label).size(theme::font(state, 12.0)),
            super::Position::Bottom,
        )
    };
    container(row![
        toggle(icons::LIST, !grid, Msg::ShowList, "List"),
        toggle(icons::GRID, grid, Msg::ShowGrid, "Cover grid"),
    ])
    .style(|_| container::Style {
        border: Border {
            color: theme::DARK.line,
            width: 1.0,
            radius: theme::RADIUS.into(),
        },
        ..Default::default()
    })
    .into()
}

// ---- the screen -------------------------------------------------------------------------------

fn header<'a>(state: &'a State, plugin: &'a PluginRow) -> Element<'a, Message> {
    let mut bar = row![].spacing(8).align_y(Alignment::Center);
    if let Some(switch) = switcher(state) {
        bar = bar.push(switch);
    }
    bar = bar.push(
        text_input(
            &format!("Search {} · press Enter", plugin.name),
            state.remote.text(&plugin.id),
        )
        .id(crate::app::search_id())
        .on_input(|text| msg(Msg::QueryChanged(text)))
        .on_submit(msg(Msg::Submit))
        .padding([6, 10])
        .size(theme::font(state, 13.0))
        .style(theme::input)
        .width(Fill),
    );
    bar = bar.push(view_switch(state));
    let mut column = column![bar].spacing(6);
    if let Some(hint) = hint_line(state, plugin) {
        column = column.push(hint);
    }
    if let Some(session) = &state.remote.session
        && matches!(session.phase, Phase::Results)
    {
        column = column.push(muted(
            state,
            format!(
                "{} results{} for “{}” · {}",
                session.hits.len(),
                if session.next.is_some() {
                    " so far"
                } else {
                    ""
                },
                session.query,
                plugin.name
            ),
        ));
    }
    container(column)
        .padding(iced::Padding {
            top: 6.0 + theme::HEADER_TOP,
            right: 12.0,
            bottom: 6.0,
            left: 12.0,
        })
        .into()
}

pub fn view(state: &State) -> Element<'_, Message> {
    let Some(plugin) = remote::active(state) else {
        return Space::new().into();
    };
    let body: Element<'_, Message> = match &state.remote.session {
        None => message_card(
            state,
            format!("Type a query and press Enter to search {}.", plugin.name),
        ),
        Some(session) => match &session.phase {
            Phase::Idle(text) => message_card(
                state,
                text.clone().unwrap_or_else(|| {
                    format!("Type a query and press Enter to search {}.", plugin.name)
                }),
            ),
            Phase::NeedsAddress => needs_address(state, plugin),
            Phase::Loading => loading(state, plugin, &session.query),
            Phase::Failed(error) => failed(state, plugin, error),
            Phase::Empty => empty(state, plugin, &session.query),
            Phase::Results => results(state),
        },
    };
    let body = iced::widget::sensor(body)
        .on_show(Message::Viewport)
        .on_resize(Message::Viewport);
    column![header(state, plugin), body].height(Fill).into()
}

fn message_card<'a>(state: &State, text: String) -> Element<'a, Message> {
    container(small(state, text))
        .padding(12)
        .center_x(Fill)
        .center_y(Fill)
        .into()
}

fn needs_address<'a>(state: &State, plugin: &PluginRow) -> Element<'a, Message> {
    container(
        column![
            text(format!("{} needs an address first", plugin.name)).size(theme::font(state, 13.0)),
            small(
                state,
                "Enter the catalog address in Settings → Plugins before searching."
            ),
            button("Open plugin settings")
                .style(theme::primary)
                .padding([6, 12])
                .on_press(msg(Msg::OpenPluginSettings(plugin.id.clone()))),
        ]
        .spacing(10)
        .align_x(Alignment::Center),
    )
    .padding(24)
    .center_x(Fill)
    .center_y(Fill)
    .into()
}

fn skeleton<'a>() -> Element<'a, Message> {
    container(Space::new())
        .width(Fill)
        .height(36)
        .style(|_| container::Style {
            background: Some(theme::DARK.hover.into()),
            border: Border {
                radius: 5.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

fn loading<'a>(state: &State, plugin: &PluginRow, query: &str) -> Element<'a, Message> {
    column![
        container(column![skeleton(), skeleton(), skeleton()].spacing(4)).padding([0, 12]),
        container(
            row![
                small(state, format!("Searching {} for “{}”…", plugin.name, query)),
                button(text("Cancel").size(theme::font(state, 12.0)))
                    .style(theme::outline)
                    .padding([3, 9])
                    .on_press(msg(Msg::Cancel)),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
        )
        .padding(12)
        .center_x(Fill),
    ]
    .spacing(8)
    .into()
}

/// The words of a failure, as the mockup has them: "<plugin>: <what happened>". A failure to
/// connect names the host and points at the address in the plugin's settings.
fn failure_text(plugin: &PluginRow, error: &SourceError) -> String {
    match error.kind {
        SourceErrorKind::Unreachable => format!(
            "{}: Could not connect to {}. Check the address in the plugin settings.",
            plugin.name,
            remote::host_of(plugin)
        ),
        SourceErrorKind::Server => format!(
            "{}: The server answered with an error. Try again later.",
            plugin.name
        ),
        _ => format!("{}: {}", plugin.name, error.message),
    }
}

fn failed<'a>(state: &State, plugin: &PluginRow, error: &SourceError) -> Element<'a, Message> {
    let mut actions = row![
        button("Try again")
            .style(theme::primary)
            .padding([6, 12])
            .on_press(msg(Msg::Retry))
    ]
    .spacing(8);
    if matches!(
        error.kind,
        SourceErrorKind::Unreachable | SourceErrorKind::Unavailable
    ) {
        actions = actions.push(
            button("Open plugin settings")
                .style(theme::outline)
                .padding([6, 12])
                .on_press(msg(Msg::OpenPluginSettings(plugin.id.clone()))),
        );
    }
    container(
        column![
            text(failure_text(plugin, error)).size(theme::font(state, 13.0)),
            actions
        ]
        .spacing(10)
        .align_x(Alignment::Center),
    )
    .padding(24)
    .center_x(Fill)
    .center_y(Fill)
    .into()
}

fn empty<'a>(state: &State, plugin: &PluginRow, query: &str) -> Element<'a, Message> {
    container(
        column![
            small(
                state,
                format!("No results for “{}” in {}.", query, plugin.name)
            ),
            muted(state, "Check the spelling or try fewer words."),
        ]
        .spacing(4)
        .align_x(Alignment::Center),
    )
    .padding(34)
    .center_x(Fill)
    .center_y(Fill)
    .into()
}

// ---- results ----------------------------------------------------------------------------------

/// `EPUB · AZW3 · FB2`: the formats of the offers, once each.
fn format_labels(hit: &RemoteHit) -> String {
    let mut labels: Vec<&str> = Vec::new();
    for offer in &hit.offers {
        if !labels.contains(&offer.format.as_str()) {
            labels.push(&offer.format);
        }
    }
    labels.join(" · ")
}

/// The cover of a result at the given size, or the grey placeholder with a book (a result
/// that gave no cover, or whose cover could not be loaded).
fn cover<'a>(
    state: &'a State,
    hit: &'a RemoteHit,
    width: f32,
    height: f32,
) -> Element<'a, Message> {
    if let Some(handle) = hit
        .cover_url
        .as_deref()
        .and_then(|url| state.remote.covers.get(url))
    {
        return container(
            image(handle.clone())
                .content_fit(iced::ContentFit::Contain)
                .width(width)
                .height(height),
        )
        .width(width)
        .height(height)
        .into();
    }
    let color = super::book_grid::placeholder_color(&hit.title);
    container(
        text(super::book_list::ellipsis(
            &hit.title,
            (width - 10.0).max(0.0),
            theme::font(state, 10.0),
        ))
        .size(theme::font(state, 10.0)),
    )
    .padding(5)
    .width(width)
    .height(height)
    .align_y(iced::alignment::Vertical::Bottom)
    .style(move |_| container::Style {
        background: Some(color.into()),
        text_color: Some(iced::Color::WHITE),
        border: Border::default().rounded(5),
        ..Default::default()
    })
    .into()
}

fn footer<'a>(state: &State, plugin: &PluginRow) -> Element<'a, Message> {
    let Some(session) = &state.remote.session else {
        return Space::new().into();
    };
    let body: Element<'a, Message> = match &session.more {
        More::Loading => small(state, "Loading more…").into(),
        More::Failed(error) => row![
            text(format!(
                "Could not load more results: {}",
                failure_text(plugin, error)
            ))
            .size(theme::font(state, 12.0))
            .color(theme::DARK.warning),
            button(text("Try again").size(theme::font(state, 12.0)))
                .style(theme::outline)
                .padding([3, 9])
                .on_press(msg(Msg::ShowMore)),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into(),
        More::Ready if session.next.is_some() => {
            button(text("Show more").size(theme::font(state, 12.0)))
                .style(theme::outline)
                .padding([3, 9])
                .on_press(msg(Msg::ShowMore))
                .into()
        }
        More::Ready => muted(state, "End of results").into(),
    };
    container(body)
        .height(FOOTER_HEIGHT)
        .padding(12)
        .center_x(Fill)
        .center_y(FOOTER_HEIGHT)
        .into()
}

/// The room under the last row for "Show more" and what replaces it.
const FOOTER_HEIGHT: f32 = 56.0;

fn results(state: &State) -> Element<'_, Message> {
    let Some(plugin) = remote::active(state) else {
        return Space::new().into();
    };
    if state.remote.grid {
        grid(state, plugin)
    } else {
        list(state, plugin)
    }
}

fn list<'a>(state: &'a State, plugin: &'a PluginRow) -> Element<'a, Message> {
    responsive(move |size| {
        let hits = state
            .remote
            .session
            .as_ref()
            .map_or(&[][..], |s| &s.hits[..]);
        let slice = crate::catalog::visible_slice_with_height(
            hits.len(),
            state.list_offset,
            size.height,
            state.row_height(),
        );
        let mut rows =
            column![Space::new().height(slice.start as f32 * state.row_height())].spacing(0);
        let available = (size.width - 26.0 - 84.0 - 24.0 - 24.0 - 12.0).max(0.0);
        let (title_width, author_width) = (available * 0.6, available * 0.4);
        let font = theme::font(state, 13.0);
        for hit in &hits[slice.clone()] {
            let selected = state.remote.selected.as_deref() == Some(hit.id.as_str());
            let focused = selected && state.keyboard_focus;
            let labels = format_labels(hit);
            let cell = row![
                cover(state, hit, 22.0, 30.0),
                text(super::book_list::ellipsis(&hit.title, title_width, font))
                    .size(font)
                    .shaping(iced::widget::text::Shaping::Advanced)
                    .width(Length::Fixed(title_width)),
                text(super::book_list::ellipsis(
                    &hit.authors.join(", "),
                    author_width,
                    font
                ))
                .size(font)
                .shaping(iced::widget::text::Shaping::Advanced)
                .color(theme::DARK.text_secondary)
                .width(Length::Fixed(author_width)),
                super::book_list::format_tags(state, &labels),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            rows = rows.push(
                button(cell)
                    .height(state.row_height())
                    .width(Fill)
                    .padding([0, 12])
                    .style(move |_, status| theme::book_row(selected, focused, status))
                    .on_press(msg(Msg::Select(hit.id.clone()))),
            );
        }
        rows = rows
            .push(Space::new().height((hits.len() - slice.end) as f32 * state.row_height()))
            .push(footer(state, plugin));
        scrollable(rows)
            .id(super::book_list::id())
            .height(Fill)
            .direction(theme::vbar())
            .style(theme::scroll(state))
            .on_scroll(|v| Message::ListScrolled {
                offset: v.absolute_offset().y,
                height: v.bounds().height,
            })
            .into()
    })
    .into()
}

fn grid<'a>(state: &'a State, plugin: &'a PluginRow) -> Element<'a, Message> {
    responsive(move |size| {
        let hits = state
            .remote
            .session
            .as_ref()
            .map_or(&[][..], |s| &s.hits[..]);
        let geometry = super::book_grid::Geometry::for_state(state);
        let cols = geometry.columns(size.width);
        let range = geometry.book_range(hits.len(), size.width, state.list_offset, size.height);
        let mut rows =
            column![Space::new().height(range.start.div_ceil(cols) as f32 * geometry.row_height)]
                .spacing(0);
        for chunk in hits[range.clone()].chunks(cols) {
            let mut cards = row![].spacing(super::book_grid::GAP);
            for hit in chunk {
                let selected = state.remote.selected.as_deref() == Some(hit.id.as_str());
                let focused = selected && state.keyboard_focus;
                let card = column![
                    cover(state, hit, geometry.cover_width, geometry.cover_height),
                    text(super::book_list::ellipsis(
                        &hit.title,
                        geometry.cover_width,
                        theme::font(state, 13.0)
                    ))
                    .size(theme::font(state, 13.0)),
                    text(super::book_list::ellipsis(
                        &hit.authors.join(", "),
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
                        .style(move |_, status| theme::book_row(selected, focused, status))
                        .on_press(msg(Msg::Select(hit.id.clone()))),
                );
            }
            rows = rows.push(
                container(cards)
                    .width(Fill)
                    .height(geometry.row_height)
                    .padding([0, 12]),
            );
        }
        rows = rows
            .push(Space::new().height(
                (hits.len().div_ceil(cols) - range.end.div_ceil(cols)) as f32 * geometry.row_height,
            ))
            .push(footer(state, plugin));
        scrollable(rows.width(Fill))
            .id(super::book_list::id())
            .width(Fill)
            .height(Fill)
            .direction(theme::vbar())
            .style(theme::scroll(state))
            .on_scroll(|v| Message::ListScrolled {
                offset: v.absolute_offset().y,
                height: v.bounds().height,
            })
            .into()
    })
    .into()
}

// ---- the details pane (G5) --------------------------------------------------------------------

fn size_text(size: Option<u64>) -> String {
    match size {
        None => "size unknown".into(),
        Some(bytes) if bytes < 1_000_000 => format!("{} KB", (bytes as f64 / 1e3).round().max(1.0)),
        Some(bytes) => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

fn offer_row<'a>(state: &State, offer: &RemoteOffer) -> Element<'a, Message> {
    let tag = if offer.zip {
        format!("{} zip", offer.format)
    } else {
        offer.format.clone()
    };
    let what = match &offer.label {
        Some(label) => format!("{label} · {}", size_text(offer.size)),
        None => size_text(offer.size),
    };
    row![
        container(text(tag).size(theme::font(state, 10.0)))
            .padding([1, 4])
            .style(theme::format_tag),
        small(state, what).width(Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// The pane to the right of the results: the selected result's cover, fields, description and
/// the files its source offers.
pub fn details(state: &State) -> Element<'_, Message> {
    let Some(plugin) = remote::active(state) else {
        return Space::new().into();
    };
    let Some(hit) = state.remote.selected_hit() else {
        return text("Select a result to see its details and download options.")
            .size(theme::font(state, 12.0))
            .color(theme::DARK.text_secondary)
            .into();
    };
    let cover_width =
        (state.visible_settings().cover_size as f32).min(state.details_width() - 24.0);
    let mut content = column![].spacing(10).width(Fill);
    content =
        content.push(container(cover(state, hit, cover_width, cover_width * 1.375)).center_x(Fill));
    content = content.push(
        text(hit.title.clone())
            .size(theme::font(state, 16.0))
            .width(Fill),
    );
    content = content.push(muted(state, format!("From {}", plugin.name)));
    let series = hit.series.as_ref().map(|series| match hit.series_index {
        Some(index) => format!("{series} · {}", index_text(index)),
        None => series.clone(),
    });
    let fields = [
        (
            "Author",
            (!hit.authors.is_empty()).then(|| hit.authors.join(", ")),
        ),
        ("Series", series),
        ("Publisher", hit.publisher.clone()),
        ("Year", hit.year.map(|y| y.to_string())),
        ("Language", hit.language.clone()),
        ("ISBN", hit.isbn.clone()),
    ];
    for (label, value) in fields {
        if let Some(value) = value {
            content = content.push(
                column![
                    muted(state, label),
                    text(value).size(theme::font(state, 12.0)).width(Fill)
                ]
                .spacing(2),
            );
        }
    }
    if !hit.tags.is_empty() {
        let mut tags = iced::widget::Row::new().spacing(4);
        for tag in &hit.tags {
            tags = tags.push(
                container(text(tag.clone()).size(theme::font(state, 10.0)))
                    .padding([1, 4])
                    .style(theme::format_tag),
            );
        }
        content = content.push(tags.wrap());
    }
    // What the search gave is shown at once; `details` adds to it, or fails and says so.
    match &state.remote.details {
        Some(DetailsPhase::Loading) => {
            content = content.push(small(state, "Loading details…"));
        }
        Some(DetailsPhase::Failed(error)) => {
            content = content.push(details_failed(state, plugin, error));
        }
        _ => {}
    }
    if let Some(description) = &hit.description {
        content = content.push(small(state, description.clone()).width(Fill));
    }
    if let Some(url) = &hit.page_url {
        content = content.push(
            button(
                row![
                    icons::view(icons::EXTERNAL_LINK),
                    text("Open page").size(theme::font(state, 12.0))
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .style(theme::outline)
            .padding([4, 9])
            .on_press(msg(Msg::OpenPage(url.clone()))),
        );
    }
    content = content.push(container(muted(state, "DOWNLOADS")).padding(iced::Padding {
        top: 6.0,
        ..Default::default()
    }));
    if hit.offers.is_empty() {
        content = content.push(
            text("This source offers no downloadable file for this book.")
                .size(theme::font(state, 12.0))
                .color(theme::DARK.text_muted),
        );
    } else {
        for offer in &hit.offers {
            content = content.push(offer_row(state, offer));
        }
    }
    scrollable(content)
        .id(super::details::content_id())
        .direction(theme::vbar())
        .style(theme::scroll(state))
        .height(Fill)
        .into()
}

fn index_text(index: f64) -> String {
    if index.fract() == 0.0 {
        format!("#{}", index as i64)
    } else {
        format!("#{index}")
    }
}

fn details_failed<'a>(
    state: &State,
    plugin: &PluginRow,
    error: &SourceError,
) -> Element<'a, Message> {
    container(
        column![
            text("Could not load more details")
                .size(theme::font(state, 13.0))
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..iced::Font::DEFAULT
                }),
            small(state, failure_text(plugin, error)),
            small(state, "Showing only what the search returned."),
            button(text("Try again").size(theme::font(state, 12.0)))
                .style(theme::outline)
                .padding([3, 9])
                .on_press(msg(Msg::RetryDetails)),
        ]
        .spacing(6),
    )
    .padding(12)
    .width(Fill)
    .style(|theme| {
        let mut style = theme::card(theme);
        style.background = Some(theme::DARK.background.into());
        style.border.radius = 6.0.into();
        style
    })
    .into()
}
