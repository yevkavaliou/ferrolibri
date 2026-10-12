use crate::{
    app::{Filter, Message, Screen, State},
    icons, theme,
};
use iced::{
    Alignment, Element, Fill,
    widget::{Space, button, column, container, row, text, tooltip},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub sidebar: bool,
    pub details: bool,
    pub details_available: bool,
    pub details_width: f32,
}
impl Layout {
    pub fn for_state(state: &State) -> Self {
        let setup = state.screen == Screen::Setup || state.library_loading;
        let details_available = !setup && matches!(state.screen, Screen::Home { .. });
        Self {
            sidebar: !setup && state.visible_settings().sidebar_visible,
            details: details_available && state.visible_settings().details_visible,
            details_available,
            details_width: state.details_width() + theme::DETAILS_GRIP,
        }
    }
    pub fn content_width(self, width: f32) -> f32 {
        width
            - if self.sidebar {
                theme::SIDEBAR_WIDTH
            } else {
                0.0
            }
            - if self.details {
                self.details_width
            } else {
                0.0
            }
    }
}
fn icon_button(
    state: &State,
    icon: &'static [u8],
    label: &str,
    message: Option<Message>,
    active: bool,
) -> Element<'static, Message> {
    let color = if message.is_none() {
        theme::DARK.text_muted.scale_alpha(0.3)
    } else if active {
        theme::DARK.accent
    } else {
        theme::DARK.text_secondary
    };
    tooltip(
        button(icons::tinted(icon, color))
            .padding([4, 6])
            .style(theme::quiet)
            .on_press_maybe(message),
        text(label.to_string()).size(theme::font(state, 12.0)),
        tooltip::Position::Bottom,
    )
    .into()
}
fn vertical_line() -> Element<'static, Message> {
    container(Space::new())
        .width(theme::LINE_WIDTH)
        .height(Fill)
        .style(theme::line)
        .into()
}
fn nav(
    state: &State,
    icon: &'static [u8],
    label: &'static str,
    screen: Screen,
    active: bool,
) -> Element<'static, Message> {
    button(
        row![
            icons::view(icon),
            text(label).size(theme::font(state, 13.0))
        ]
        .spacing(theme::SPACE)
        .align_y(Alignment::Center),
    )
    .width(Fill)
    .padding([6, 8])
    .style(move |_, status| {
        theme::button_style(
            if active {
                theme::Button::Selected
            } else {
                theme::Button::Quiet
            },
            status,
        )
    })
    .on_press(Message::Navigate(screen))
    .into()
}
fn group(
    state: &State,
    label: &'static str,
    expanded: bool,
    group: crate::app::FacetGroup,
    indent: f32,
) -> Element<'static, Message> {
    let mut label_row = row![].spacing(8).align_y(Alignment::Center);
    if matches!(group, crate::app::FacetGroup::Library) {
        label_row = label_row.push(icons::view(icons::BOOKS));
    }
    label_row = label_row
        .push(text(label).size(theme::font(state, 13.0)).width(Fill))
        .push(icons::view(if expanded {
            icons::CHEVRON_DOWN
        } else {
            icons::CHEVRON_RIGHT
        }));
    button(label_row)
        .padding(iced::Padding {
            left: indent,
            right: 8.0,
            top: 6.0,
            bottom: 6.0,
        })
        .width(Fill)
        .style(theme::quiet)
        .on_press(Message::ToggleFacetGroup(group))
        .into()
}
fn facet<'a>(
    state: &State,
    label: &'a str,
    count: usize,
    filter: Filter,
    active: bool,
) -> Element<'a, Message> {
    tooltip(
        button(
            row![
                text(super::book_list::ellipsis(
                    label,
                    62.0,
                    theme::font(state, 12.0)
                ))
                .size(theme::font(state, 12.0))
                .width(Fill),
                text(count.to_string())
                    .size(theme::font(state, 10.0))
                    .color(theme::DARK.text_muted)
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .width(Fill)
        .padding(iced::Padding {
            left: 30.0,
            right: 8.0,
            top: 5.0,
            bottom: 5.0,
        })
        .style(move |_, status| {
            theme::button_style(
                if active {
                    theme::Button::Selected
                } else {
                    theme::Button::Quiet
                },
                status,
            )
        })
        .on_press(Message::Navigate(Screen::Home { filter })),
        text(label).size(theme::font(state, 12.0)),
        tooltip::Position::Right,
    )
    .into()
}
fn sidebar(state: &State) -> Element<'_, Message> {
    use crate::app::FacetGroup;
    let mut items = column![
        nav(
            state,
            icons::HOME,
            "Home",
            Screen::Home {
                filter: Filter::All
            },
            state.screen
                == Screen::Home {
                    filter: Filter::All
                }
        ),
        tooltip(
            nav(
                state,
                icons::SEARCH,
                "Search",
                Screen::Search,
                state.screen == Screen::Search
            ),
            text(crate::keymap::tip(
                "Search",
                crate::keymap::Command::OpenSearch
            ))
            .size(theme::font(state, 12.0)),
            tooltip::Position::Right,
        ),
        group(
            state,
            "Library",
            state.sidebar_expansion.library,
            FacetGroup::Library,
            8.0
        ),
    ]
    .spacing(1);
    if state.sidebar_expansion.library {
        items = items.push(group(
            state,
            "Formats",
            state.sidebar_expansion.formats,
            FacetGroup::Formats,
            26.0,
        ));
        if state.sidebar_expansion.formats {
            for (format, count) in &state.catalog.formats {
                let filter = Filter::Format(*format);
                let active = state.screen
                    == Screen::Home {
                        filter: filter.clone(),
                    };
                items = items.push(facet(state, format.label(), *count, filter, active));
            }
        }
        items = items.push(group(
            state,
            "Authors",
            state.sidebar_expansion.authors,
            FacetGroup::Authors,
            26.0,
        ));
        if state.sidebar_expansion.authors {
            for (normalized, (label, count)) in &state.catalog.authors {
                let active = matches!(&state.screen, Screen::Home {filter: Filter::Author(a)} if a.to_lowercase() == *normalized);
                items = items.push(facet(
                    state,
                    label,
                    *count,
                    Filter::Author(label.clone()),
                    active,
                ));
            }
        }
    }
    let unfinished = state
        .jobs
        .values()
        .filter(|j| {
            matches!(
                j.state,
                rulibre_core::jobs::JobState::Waiting | rulibre_core::jobs::JobState::Running
            )
        })
        .count();
    let jobs = button(
        row![
            icons::view(icons::LIST_CHECK),
            text("Jobs").size(theme::font(state, 13.0)).width(Fill),
            text(if unfinished > 0 {
                unfinished.to_string()
            } else {
                String::new()
            })
            .size(theme::font(state, 11.0))
            .color(theme::DARK.accent)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .width(Fill)
    .padding([6, 8])
    .style(move |_, status| {
        theme::button_style(
            if state.screen == Screen::Jobs {
                theme::Button::Selected
            } else {
                theme::Button::Quiet
            },
            status,
        )
    })
    .on_press(Message::Navigate(Screen::Jobs));
    items = items.push(jobs).push(nav(
        state,
        icons::SETTINGS,
        "Settings",
        Screen::Settings,
        state.screen == Screen::Settings,
    ));
    row![
        container(
            iced::widget::scrollable(items)
                .direction(theme::vbar())
                .style(theme::scroll(state))
        )
        .padding([4, 6])
        .width(Fill)
        .height(Fill),
        vertical_line()
    ]
    .width(theme::SIDEBAR_WIDTH)
    .height(Fill)
    .into()
}
fn details(state: &State) -> Element<'_, Message> {
    let alpha = crate::app::blur_alpha(state);
    let panel = if alpha < 1.0 {
        crate::blur::GLASS_PANEL
    } else {
        theme::DARK.panel
    };
    let dragging = state.details_drag.is_some();
    row![
        // The whole grip stays draggable; only a 1 px line is drawn (a 0.5 px quad
        // on a fractional position vanishes), 2 px in the accent colour while dragging.
        iced::widget::mouse_area(
            container(
                container(Space::new())
                    .width(if dragging { 2.0 } else { 1.0 })
                    .height(Fill)
                    .style(move |_| container::Style {
                        background: Some(
                            if dragging {
                                theme::DARK.accent
                            } else {
                                theme::DARK.line
                            }
                            .into()
                        ),
                        ..Default::default()
                    })
            )
            .align_right(theme::DETAILS_GRIP)
            .height(Fill)
        )
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .on_press(Message::StartDetailsResize),
        container(super::details::view(state))
            .padding([14, 12])
            .width(Fill)
            .height(Fill)
            .style(move |theme| {
                let mut style = theme::panel(theme);
                style.background = Some(panel.scale_alpha(alpha).into());
                style
            })
    ]
    .width(state.details_width() + theme::DETAILS_GRIP)
    .height(Fill)
    .into()
}
pub fn view<'a>(state: &'a State, content: Element<'a, Message>) -> Element<'a, Message> {
    let layout = Layout::for_state(state);
    let custom = state.blur.ready;
    let sidebar_button = icon_button(
        state,
        icons::SIDEBAR,
        &crate::keymap::tip("Toggle sidebar", crate::keymap::Command::ToggleSidebar),
        (state.screen != Screen::Setup && !state.library_loading).then_some(Message::ToggleSidebar),
        layout.sidebar,
    );
    let details_button = icon_button(
        state,
        icons::SIDEBAR_RIGHT,
        &crate::keymap::tip("Toggle details pane", crate::keymap::Command::ToggleDetails),
        layout.details_available.then_some(Message::ToggleDetails),
        layout.details,
    );
    let title = text("Rulibre")
        .size(theme::font(state, 12.0))
        .color(theme::DARK.text_muted);
    let toolbar: Element<'_, Message> = if custom {
        // Own title bar: the drag area is the empty strip between the buttons.
        let drag = iced::widget::mouse_area(
            row![title, Space::new().width(Fill)]
                .spacing(theme::SPACE)
                .align_y(Alignment::Center)
                .width(Fill)
                .height(Fill),
        )
        .on_press(Message::WindowDrag)
        .on_double_click(Message::WindowToggleMaximize);
        row![
            sidebar_button,
            drag,
            details_button,
            icon_button(
                state,
                icons::MINIMIZE,
                "Minimize",
                Some(Message::WindowMinimize),
                false
            ),
            icon_button(
                state,
                icons::MAXIMIZE,
                "Maximize",
                Some(Message::WindowToggleMaximize),
                false
            ),
            icon_button(state, icons::X, "Close", Some(Message::WindowClose), false),
        ]
        .spacing(theme::SPACE)
        .align_y(Alignment::Center)
        .into()
    } else {
        row![
            sidebar_button,
            title,
            Space::new().width(Fill),
            details_button
        ]
        .spacing(theme::SPACE)
        .align_y(Alignment::Center)
        .into()
    };
    let mut main = row![].height(Fill).width(Fill);
    if layout.sidebar {
        main = main.push(sidebar(state));
    }
    main = main.push(container(content).width(Fill).height(Fill));
    if layout.details {
        main = main.push(details(state));
    }
    let bottom = column![
        container(Space::new())
            .width(Fill)
            .height(theme::LINE_WIDTH)
            .style(theme::line),
        super::device_bar::bar(state),
    ]
    .height(theme::BOTTOM_HEIGHT);
    let ui = container(
        column![
            container(toolbar)
                .padding([4, 8])
                .height(theme::TOOLBAR_HEIGHT)
                .width(Fill),
            // Same hairline as above the device bar.
            container(Space::new())
                .width(Fill)
                .height(theme::LINE_WIDTH)
                .style(theme::line),
            main,
            bottom,
        ]
        .height(Fill),
    )
    .width(Fill)
    .height(Fill)
    .style(|_| container::Style {
        text_color: Some(theme::DARK.text_primary),
        ..Default::default()
    });
    let alpha = crate::app::blur_alpha(state);
    let background = if alpha < 1.0 {
        crate::blur::GLASS_BACKGROUND
    } else {
        theme::DARK.background
    };
    let base = iced::widget::stack![
        container(Space::new())
            .width(Fill)
            .height(Fill)
            .style(move |theme| {
                let mut style = theme::surface(theme);
                style.background = Some(background.scale_alpha(alpha).into());
                style
            }),
        ui
    ]
    .width(Fill)
    .height(Fill);
    let base: Element<'_, Message> = if state.reader_picker.open {
        iced::widget::stack![
            base,
            iced::widget::mouse_area(container(Space::new()).width(Fill).height(Fill))
                .on_press(Message::CloseReaderMenu),
            container(
                iced::widget::mouse_area(super::device_bar::menu(state))
                    .on_press(Message::ReaderMenuInside)
            )
            .padding(iced::Padding {
                top: 0.0,
                right: 12.0,
                bottom: theme::BOTTOM_HEIGHT + 4.0,
                left: 12.0
            })
            .width(Fill)
            .height(Fill)
            .align_bottom(Fill),
        ]
        .into()
    } else {
        base.into()
    };
    let base = if state.export_prompt {
        iced::widget::stack![
            base,
            iced::widget::mouse_area(container(Space::new()).width(Fill).height(Fill).style(
                |_| container::Style {
                    background: Some(iced::Color::BLACK.scale_alpha(0.45).into()),
                    ..Default::default()
                }
            ))
            .on_press(Message::Ignore),
            container(super::details::export_prompt(state))
                .width(Fill)
                .height(Fill)
                .center_x(Fill)
                .center_y(Fill)
        ]
        .into()
    } else {
        base
    };
    let base: Element<'_, Message> = if !state.toasts.is_empty() || !state.notices.is_empty() {
        iced::widget::stack![
            base,
            container(super::toasts::view(state))
                .padding(iced::Padding {
                    top: 0.0,
                    right: 12.0,
                    bottom: theme::BOTTOM_HEIGHT + 12.0,
                    left: 12.0,
                })
                .width(Fill)
                .height(Fill)
                .align_right(Fill)
                .align_bottom(Fill)
        ]
        .into()
    } else {
        base
    };
    if state.blur.ready {
        iced::widget::stack![base, resize_frame()].into()
    } else {
        base
    }
}
/// Invisible edge and corner handles: an undecorated window has no resize border.
fn resize_frame() -> Element<'static, Message> {
    use iced::{mouse::Interaction, window::Direction};
    const GRIP: f32 = 5.0;
    let handle =
        |direction: Direction, cursor: Interaction, width: iced::Length, height: iced::Length| {
            iced::widget::mouse_area(Space::new().width(width).height(height))
                .interaction(cursor)
                .on_press(Message::WindowResize(direction))
        };
    let fixed = iced::Length::Fixed(GRIP);
    column![
        row![
            handle(
                Direction::NorthWest,
                Interaction::ResizingDiagonallyDown,
                fixed,
                fixed
            ),
            handle(
                Direction::North,
                Interaction::ResizingVertically,
                Fill,
                fixed
            ),
            handle(
                Direction::NorthEast,
                Interaction::ResizingDiagonallyUp,
                fixed,
                fixed
            ),
        ],
        row![
            handle(
                Direction::West,
                Interaction::ResizingHorizontally,
                fixed,
                Fill
            ),
            Space::new().width(Fill).height(Fill),
            handle(
                Direction::East,
                Interaction::ResizingHorizontally,
                fixed,
                Fill
            ),
        ]
        .height(Fill),
        row![
            handle(
                Direction::SouthWest,
                Interaction::ResizingDiagonallyUp,
                fixed,
                fixed
            ),
            handle(
                Direction::South,
                Interaction::ResizingVertically,
                Fill,
                fixed
            ),
            handle(
                Direction::SouthEast,
                Interaction::ResizingDiagonallyDown,
                fixed,
                fixed
            ),
        ],
    ]
    .width(Fill)
    .height(Fill)
    .into()
}
