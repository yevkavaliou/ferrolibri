use crate::{
    app::{Message, Screen, State},
    icons, theme,
};
use iced::{
    Alignment, Element, Fill,
    widget::{
        Space, button, column, container, pick_list, row, scrollable, text, text_input, tooltip,
    },
};
use rulibre_core::device::DeviceKind;

pub fn space_label(bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) => format!("{:.1} GB free", bytes as f64 / 1_000_000_000.0),
        None => "free space unknown".into(),
    }
}
pub fn bar(state: &State) -> Element<'_, Message> {
    iced::widget::responsive(move |size| {
        let readers = &state.readers;
        let selected = readers.selected();
        let multiple = readers.devices.len() > 1;
        let running = state.jobs.values().find(|j| {
            j.kind == crate::services::jobs::JobKind::Send
                && j.state == rulibre_core::jobs::JobState::Running
                && selected.is_none_or(|d| state.send_devices.get(&j.id) == Some(&d.id))
        });
        let compact = running.is_some() && size.width < 1000.0;
        let color = if selected.is_some() {
            theme::DARK.ok
        } else if multiple {
            theme::DARK.warning
        } else {
            theme::DARK.text_muted
        };
        let label = selected.map(|d| d.name().to_owned()).unwrap_or_else(|| {
            if multiple {
                format!("{} readers found · choose one", readers.devices.len())
            } else {
                "No reader connected".into()
            }
        });
        let title_width = if compact {
            110.0
        } else if multiple {
            224.0
        } else if selected.is_none() {
            // Nothing else sits beside the label, so "No reader connected" fits whole.
            200.0
        } else {
            148.0
        };
        let mut name = row![
            text("●").size(theme::font(state, 10.0)).color(color),
            text(super::book_list::ellipsis(
                &label,
                title_width - 38.0,
                theme::font(state, 12.0)
            ))
            .size(theme::font(state, 12.0))
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if multiple {
            name = name.push(icons::tinted(icons::CHEVRON_DOWN, theme::DARK.text_primary));
        }
        let name: Element<'_, Message> = if multiple {
            tooltip(
                button(name)
                    .style(theme::quiet)
                    .padding([4, 6])
                    .on_press(Message::ToggleReaderMenu),
                text("Choose reader").size(theme::font(state, 12.0)),
                tooltip::Position::Top,
            )
            .into()
        } else {
            name.into()
        };
        let mut contents = row![name].spacing(8).align_y(Alignment::Center);
        if let Some(reader) = selected {
            let connection = if reader.is_mtp() {
                "USB / MTP".to_owned()
            } else {
                reader.mount_point.display().to_string()
            };
            let full = if reader.is_mtp() {
                reader.label.clone().unwrap_or_else(|| connection.clone())
            } else {
                format!("{} · {}", connection, space_label(readers.available()))
            };
            // Leave room for name, warning, manual icon, and Send at minimum width.
            let width =
                (size.width - title_width - 430.0 - if running.is_some() { 250.0 } else { 0.0 })
                    .clamp(0.0, 380.0);
            let mut location = row![]
                .spacing(6)
                .align_y(Alignment::Center)
                .width(width + 150.0);
            if width >= 30.0 {
                location = location.push(
                    text(super::book_list::ellipsis(
                        &connection,
                        width,
                        theme::font(state, 12.0),
                    ))
                    .size(theme::font(state, 12.0))
                    .width(width)
                    .color(theme::DARK.text_secondary),
                );
            }
            if !reader.is_mtp() {
                location = location.push(
                    text(format!("· {}", space_label(readers.available())))
                        .size(theme::font(state, 12.0))
                        .color(theme::DARK.text_secondary),
                );
            }
            contents = contents.push(tooltip(
                location,
                text(full).size(theme::font(state, 12.0)),
                tooltip::Position::Top,
            ));
            if !multiple && !reader.is_mtp() {
                contents = contents.push(tooltip(
                    button(icons::view(icons::FOLDER))
                        .style(theme::quiet)
                        .padding(3)
                        .on_press(Message::ManualReader),
                    text("Select folder manually…").size(theme::font(state, 12.0)),
                    tooltip::Position::Top,
                ));
            }
        }
        let warning = readers
            .failure
            .as_deref()
            .or_else(|| readers.warnings.first().map(String::as_str))
            .or_else(|| {
                selected
                    .filter(|d| d.read_only)
                    .map(|_| "Reader is read-only")
            });
        if let Some(warning) = warning {
            contents = contents.push(tooltip(
                text(if compact {
                    "!".into()
                } else {
                    super::book_list::ellipsis(warning, 130.0, theme::font(state, 11.0))
                })
                .size(theme::font(state, 11.0))
                .color(theme::DARK.warning),
                text(warning.to_owned()).size(theme::font(state, 12.0)),
                tooltip::Position::Top,
            ));
        }
        contents = contents.push(Space::new().width(Fill));
        if let Some(job) = running {
            let queued = state
                .jobs
                .values()
                .filter(|j| {
                    j.kind == crate::services::jobs::JobKind::Send
                        && j.state == rulibre_core::jobs::JobState::Waiting
                        && state.send_devices.get(&j.id) == state.send_devices.get(&job.id)
                })
                .count();
            let progress = super::jobs::percentage(job);
            let mut label = progress
                .map(|p| format!("{p:.0}%"))
                .unwrap_or_else(|| super::jobs::status(job));
            if queued > 0 {
                label.push_str(&format!(" · {queued} queued"));
            }
            contents = contents.push(
                text(label)
                    .size(theme::font(state, 12.0))
                    .width(if compact {
                        iced::Length::Fixed(if progress.is_some() { 104.0 } else { 115.0 })
                    } else {
                        iced::Length::Shrink
                    })
                    .color(theme::DARK.text_secondary),
            );
            if let Some(p) = progress {
                contents = contents.push(
                    iced::widget::progress_bar(0.0..=100.0, p)
                        .length(if compact { 48 } else { 64 })
                        .girth(3),
                );
            }
            contents = contents.push(
                button(text("Cancel").size(theme::font(state, 12.0)))
                    .style(theme::quiet)
                    .on_press(Message::Cancel(job.id)),
            );
        }
        if selected.is_some() {
            let count = state.selection.selected.len();
            let label = if count > 1 {
                format!("Send {count} to reader")
            } else {
                "Send to reader".into()
            };
            let enabled = readers.can_send(count)
                && state.screen != Screen::Setup
                && !state.library_loading
                && !state.closing;
            let send = button(text(label).size(theme::font(state, 12.0)))
                .style(theme::primary)
                .padding([3, 9])
                .on_press_maybe(enabled.then_some(Message::Send));
            contents = contents.push(tooltip(
                send,
                text(match send_blocker(state) {
                    Some(reason) => reason.to_string(),
                    None => crate::keymap::tip("Send to reader", crate::keymap::Command::Send),
                })
                .size(theme::font(state, 12.0)),
                tooltip::Position::Top,
            ));
        } else if multiple {
            contents = contents.push(
                text("Choose a reader to send books")
                    .size(theme::font(state, 12.0))
                    .color(theme::DARK.text_muted),
            );
        } else {
            contents = contents.push(tooltip(
                button(text("Select folder manually…").size(theme::font(state, 12.0)))
                    .style(theme::quiet)
                    .on_press(Message::ManualReader),
                text("Connect a reader to send books").size(theme::font(state, 12.0)),
                tooltip::Position::Top,
            ));
        }
        container(contents.width(Fill))
            .padding([0, 12])
            .width(Fill)
            .height(Fill)
            .center_y(Fill)
            .into()
    })
    .into()
}

/// Why Send would do nothing right now, phrased as the next step.
pub fn send_blocker(state: &State) -> Option<&'static str> {
    let readers = &state.readers;
    if state.selection.selected.is_empty() {
        Some("Select the books to send, then press T.")
    } else if readers.failure.is_some() {
        Some("The reader list could not be read. Reconnect the reader or choose its folder.")
    } else if let Some(reader) = readers.selected() {
        (!reader.can_transfer()).then_some("This reader is read-only, so books cannot be sent.")
    } else if readers.devices.len() > 1 {
        Some("Choose a reader in the bar below, then press T.")
    } else {
        Some("Connect a reader or choose its folder to send books.")
    }
}
pub fn menu(state: &State) -> Element<'_, Message> {
    let picker = &state.reader_picker;
    let mut items = column![].spacing(6);
    if picker.manual {
        items = items.push(row![
            text("Select folder manually")
                .size(theme::font(state, 13.0))
                .width(Fill),
            button(icons::view(icons::X))
                .style(theme::quiet)
                .on_press(Message::CloseReaderMenu)
        ]);
    }
    if state.readers.devices.is_empty() {
        items = items.push(
            text("No readers detected")
                .size(theme::font(state, 12.0))
                .color(theme::DARK.text_muted),
        );
    }
    for reader in &state.readers.devices {
        let active = Some(&reader.id) == state.readers.selected_id.as_ref();
        let details = format!(
            "{} · {} · {} · {}",
            reader.label.as_deref().unwrap_or("Unlabelled"),
            if reader.is_mtp() {
                "USB".into()
            } else {
                reader.mount_point.display().to_string()
            },
            reader.filesystem,
            if reader.read_only {
                "Read-only"
            } else {
                "Writable"
            }
        );
        items = items.push(
            button(
                column![
                    row![
                        if active {
                            icons::view(icons::CHECK)
                        } else {
                            icons::view(icons::BOOKS)
                        },
                        text(reader.name()).size(theme::font(state, 12.0))
                    ]
                    .spacing(6),
                    text(details)
                        .size(theme::font(state, 11.0))
                        .color(theme::DARK.text_secondary),
                ]
                .spacing(2),
            )
            .width(Fill)
            .padding(8)
            .style(theme::quiet)
            .on_press(Message::SelectReader(reader.id.clone())),
        );
    }
    items = items.push(
        button(text("Select folder manually…").size(theme::font(state, 12.0)))
            .style(theme::quiet)
            .on_press(Message::ManualReader),
    );
    if picker.manual {
        items = items.push(text("Choose the mounted reader root and its type. Kobo requires .kobo; Kindle requires documents.").size(theme::font(state, 12.0)).color(theme::DARK.text_secondary));
        items = items.push(
            row![
                text("Reader type").size(theme::font(state, 12.0)),
                pick_list(
                    ["Kobo", "Kindle"],
                    Some(if picker.kind == DeviceKind::Kobo {
                        "Kobo"
                    } else {
                        "Kindle"
                    }),
                    |kind| Message::ReaderKind(if kind == "Kobo" {
                        DeviceKind::Kobo
                    } else {
                        DeviceKind::Kindle
                    })
                )
                .text_size(12),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        items = items.push(
            text_input("Reader folder", &picker.path)
                .size(theme::font(state, 12.0))
                .style(theme::input)
                .on_input_maybe((!picker.busy).then_some(Message::ReaderPath)),
        );
        items = items.push(
            row![
                button(text("Browse…").size(theme::font(state, 12.0)))
                    .style(theme::quiet)
                    .on_press_maybe((!picker.busy).then_some(Message::ReaderBrowse)),
                button(
                    text(if picker.busy {
                        "Checking…"
                    } else {
                        "Use folder"
                    })
                    .size(theme::font(state, 12.0))
                )
                .style(theme::primary)
                .on_press_maybe(
                    (!picker.busy && !picker.path.trim().is_empty())
                        .then_some(Message::ValidateReader)
                ),
            ]
            .spacing(8),
        );
        if let Some(error) = &picker.error {
            items = items.push(
                text(error)
                    .size(theme::font(state, 12.0))
                    .color(theme::DARK.warning),
            );
        }
    }
    container(
        scrollable(items)
            .height(iced::Length::Shrink)
            .direction(theme::vbar())
            .style(theme::scroll(state)),
    )
    .padding(8)
    .width(360)
    .max_height(360)
    .style(theme::card)
    .into()
}
