//! Settings → Plugins (design gap G2) and the install dialog (G3), drawn after
//! `docs/design/mockup.html`. The texts are the mockup's.
use crate::{
    app::{Message, State},
    icons,
    plugins::{Form, Install, Mode, Msg, is_plain_http},
    services::plugins::{
        Candidate, Change, FieldKind, FieldValue, Health, PermissionSummary, PluginRow, Problem,
        ProblemKind, RELEASES_SHOWN, Releases, SettingField, USUAL_CALL_TIMEOUT_S,
    },
    theme,
};
use iced::{
    Alignment, Border, Color, Element, Fill, Font, Length,
    widget::{
        Space, button, checkbox, column, container, pick_list, row, scrollable, text, text_input,
    },
};

fn msg(m: Msg) -> Message {
    Message::Plugins(m)
}

fn strong() -> Font {
    Font {
        weight: iced::font::Weight::Bold,
        ..Font::DEFAULT
    }
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

fn heading<'a>(state: &State, label: &'static str) -> iced::widget::Text<'a> {
    muted(state, label)
}

fn badge<'a>(state: &State, label: impl Into<String>, color: Color) -> Element<'a, Message> {
    container(
        text(label.into())
            .size(theme::font(state, 10.0))
            .color(color),
    )
    .padding([1, 6])
    .style(|_| container::Style {
        background: Some(theme::DARK.hover.into()),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

fn code<'a>(state: &State, label: impl Into<String>) -> Element<'a, Message> {
    container(
        text(label.into())
            .size(theme::font(state, 12.0))
            .font(Font::MONOSPACE),
    )
    .padding([1, 5])
    .style(|_| container::Style {
        background: Some(theme::DARK.background.into()),
        border: Border {
            color: theme::DARK.line,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn bullet<'a>(state: &State, content: impl Into<String>) -> Element<'a, Message> {
    row![small(state, "•"), small(state, content).width(Fill)]
        .spacing(6)
        .into()
}

/// A box with a coloured bar on the left: why a plugin cannot be used, what to enter.
fn notice<'a>(accent: Color, body: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(
        row![
            container(Space::new())
                .width(3)
                .height(Fill)
                .style(move |_| container::Style {
                    background: Some(accent.into()),
                    ..Default::default()
                }),
            container(body).padding([8, 10]).width(Fill),
        ]
        .height(Length::Shrink),
    )
    .width(Fill)
    .style(|_| container::Style {
        background: Some(theme::DARK.background.into()),
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// The box of the design's "setup-card": a title, lines of detail.
fn card<'a>(body: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(body)
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

fn rule<'a>() -> Element<'a, Message> {
    container(Space::new())
        .width(Fill)
        .height(theme::LINE_WIDTH)
        .style(theme::line)
        .into()
}

/// What the plugin may and may not do, in the words the user reads before installing.
pub fn permissions<'a>(state: &State, p: &'a PermissionSummary) -> Element<'a, Message> {
    let mut can = column![heading(state, "THIS PLUGIN CAN")].spacing(4);
    if !p.domains.is_empty() {
        let mut sites = iced::widget::Row::new()
            .spacing(4)
            .align_y(Alignment::Center);
        sites = sites.push(small(state, "Contact these sites:"));
        for domain in &p.domains {
            sites = sites.push(code(state, domain.clone()));
        }
        sites = sites.push(small(state, "(https only)"));
        can = can.push(row![small(state, "•"), sites.wrap()].spacing(6).width(Fill));
    }
    if p.user_address {
        can = can.push(bullet(
            state,
            "Contact the address you enter in its settings, and that address’s subdomains — only after you save it",
        ));
    }
    if p.domains.is_empty() && !p.user_address {
        can = can.push(bullet(state, "Use no network at all"));
    }
    can = can.push(bullet(
        state,
        format!(
            "Run for up to {} s per search or download{}",
            p.call_timeout_s,
            if p.call_timeout_s > USUAL_CALL_TIMEOUT_S {
                " (longer than the usual 30 s)"
            } else {
                ""
            }
        ),
    ));
    if !p.content.is_empty() {
        can = can.push(bullet(
            state,
            format!(
                "Declare content: {} (stated by its author, not checked)",
                p.content.join(", ")
            ),
        ));
    }
    let cannot = column![
        heading(state, "THIS PLUGIN CANNOT"),
        bullet(state, "Read your files, run programs or see your library"),
        bullet(
            state,
            "Use the network by itself — FerroLibri makes every request"
        ),
    ]
    .spacing(4);
    column![can, cannot].spacing(12).into()
}

// ---- Settings → Plugins -------------------------------------------------------------------

pub fn group(state: &State) -> Element<'_, Message> {
    let plugins = &state.plugins;
    let mut list = column![
        container(heading(state, "PLUGINS")).padding([6, 0]),
        row![
            small(state, "Plugins add search sources to the Search screen.").width(Fill),
            button(text("Install plugin…").size(theme::font(state, 12.0)))
                .style(theme::primary)
                .on_press(msg(Msg::OpenInstall(None))),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    ]
    .spacing(8);
    if let Some(error) = &plugins.error {
        list = list.push(notice(
            theme::DARK.error,
            column![
                text("The plugins could not be listed")
                    .size(theme::font(state, 13.0))
                    .font(strong()),
                small(state, error.clone()),
            ]
            .spacing(4),
        ));
    }
    if plugins.rows.is_empty() && plugins.error.is_none() && plugins.loaded {
        list = list.push(
            container(
                column![
                    text("No plugins installed.")
                        .size(theme::font(state, 13.0))
                        .color(theme::DARK.text_muted),
                    text("Install one from a folder or a web address, for example a GitHub repository.")
                        .size(theme::font(state, 13.0))
                        .color(theme::DARK.text_muted),
                ]
                .align_x(Alignment::Center),
            )
            .padding([18, 20])
            .center_x(Fill),
        );
    }
    for plugin in &plugins.rows {
        list = list.push(plugin_row(state, plugin));
    }
    list.into()
}

fn plugin_row<'a>(state: &'a State, plugin: &'a PluginRow) -> Element<'a, Message> {
    let plugins = &state.plugins;
    let expanded = plugins.expanded.as_deref() == Some(plugin.id.as_str());
    let busy = plugins.pending.contains(&plugin.id);
    let status = match &plugin.health {
        Health::Ready if plugin.enabled => badge(state, "On", theme::DARK.ok),
        Health::Ready => badge(state, "Off", theme::DARK.text_secondary),
        Health::Incompatible(_) => badge(state, "Not compatible", theme::DARK.error),
        Health::Broken(_) => badge(state, "Cannot be read", theme::DARK.error),
    };
    let mut title = row![
        text(plugin.name.clone())
            .size(theme::font(state, 14.0))
            .font(strong())
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if !plugin.version.is_empty() {
        title = title.push(badge(
            state,
            format!("v{}", plugin.version),
            theme::DARK.text_secondary,
        ));
    }
    title = title.push(status);
    let subtitle = match (plugin.description.is_empty(), plugin.author.is_empty()) {
        (false, false) => format!("{} · by {}", plugin.description, plugin.author),
        (true, false) => format!("By {}", plugin.author),
        (false, true) => plugin.description.clone(),
        (true, true) => String::new(),
    };
    let on_toggle = |on: bool| {
        msg(Msg::SetEnabled {
            id: plugin.id.clone(),
            on,
        })
    };
    let switch = checkbox(plugin.enabled && plugin.is_usable())
        .style(theme::check)
        .on_toggle_maybe((plugin.is_usable() && !busy).then_some(on_toggle));
    let mut main = row![
        column![title, small(state, subtitle)]
            .spacing(2)
            .width(Fill),
        button(text(if expanded { "Hide" } else { "Details" }).size(theme::font(state, 12.0)))
            .style(theme::outline)
            .on_press(msg(Msg::ToggleDetails(plugin.id.clone()))),
        switch,
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if busy {
        main = main.push(small(state, "Working…"));
    }
    let mut body = column![main].spacing(8);
    match &plugin.health {
        Health::Incompatible(reasons) => {
            let mut why = column![
                text("This plugin cannot be used:")
                    .size(theme::font(state, 12.0))
                    .font(strong())
            ]
            .spacing(2);
            for reason in reasons {
                why = why.push(bullet(state, reason.clone()));
            }
            body = body.push(notice(theme::DARK.error, why));
        }
        Health::Broken(message) => {
            body = body.push(notice(
                theme::DARK.error,
                column![
                    text("This plugin cannot be used:")
                        .size(theme::font(state, 12.0))
                        .font(strong()),
                    bullet(state, message.clone()),
                ]
                .spacing(2),
            ));
        }
        Health::Ready if plugin.needs_setup() => {
            body = body.push(notice(
                theme::DARK.warning,
                small(
                    state,
                    "Enter the catalog address under Details to start searching with this plugin.",
                ),
            ));
        }
        Health::Ready => {}
    }
    if expanded {
        body = body.push(details(state, plugin));
    }
    column![container(body).padding([10, 0]), rule()].into()
}

fn details<'a>(state: &'a State, plugin: &'a PluginRow) -> Element<'a, Message> {
    let plugins = &state.plugins;
    let mut body = column![].spacing(12);
    if let Some(info) = &plugin.info {
        body = body.push(permissions(state, &info.permissions));
        if !info.settings.is_empty() {
            let form = plugins.forms.get(&plugin.id);
            let mut settings = column![heading(state, "SETTINGS")].spacing(8);
            for (index, field) in info.settings.iter().enumerate() {
                settings = settings.push(field_view(state, plugin, form, index, field));
            }
            let saving = form.is_some_and(|f| f.saving);
            settings = settings.push(
                row![
                    button(text("Save settings").size(theme::font(state, 12.0)))
                        .style(theme::primary)
                        .on_press_maybe(
                            (!saving).then(|| msg(Msg::SaveSettings(plugin.id.clone())))
                        ),
                ]
                .padding([4, 0]),
            );
            body = body.push(settings);
        }
        body = body.push(
            column![
                heading(state, "SOURCE"),
                small(
                    state,
                    format!("{} · installed {}", info.source, info.installed)
                ),
            ]
            .spacing(4),
        );
    }
    if plugins.removing.as_deref() == Some(plugin.id.as_str()) {
        body = body.push(card(
            column![
                text(format!("Remove “{}”?", plugin.name))
                    .size(theme::font(state, 14.0))
                    .font(strong()),
                small(
                    state,
                    "The plugin and its saved settings are deleted. Books you added with it stay in your library.",
                ),
                row![
                    button(text("Remove").size(theme::font(state, 12.0)))
                        .style(theme::primary)
                        .on_press(msg(Msg::ConfirmRemove(plugin.id.clone()))),
                    button(text("Keep it").size(theme::font(state, 12.0)))
                        .style(theme::outline)
                        .on_press(msg(Msg::KeepPlugin)),
                ]
                .spacing(8),
            ]
            .spacing(8),
        ));
    } else {
        let mut actions = row![].spacing(8).align_y(Alignment::Center);
        if plugin.info.is_some() {
            let mode = match plugin.info.as_ref().map(|i| i.source.as_str()) {
                Some(source) if source.starts_with("Folder") => Mode::Folder,
                _ => Mode::Address,
            };
            actions = actions.push(
                button(text("Update or reinstall…").size(theme::font(state, 12.0)))
                    .style(theme::outline)
                    .on_press(msg(Msg::OpenInstall(Some(mode)))),
            );
        }
        actions = actions.push(Space::new().width(Fill)).push(
            button(text("Remove…").size(theme::font(state, 12.0)))
                .style(theme::outline)
                .on_press(msg(Msg::AskRemove(plugin.id.clone()))),
        );
        body = body.push(actions);
    }
    body.into()
}

fn field_view<'a>(
    state: &'a State,
    plugin: &'a PluginRow,
    form: Option<&'a Form>,
    index: usize,
    field: &'a SettingField,
) -> Element<'a, Message> {
    let value = form
        .and_then(|f| f.values.get(index))
        .unwrap_or(&field.value);
    let id = plugin.id.clone();
    let edit = move |value: FieldValue| {
        msg(Msg::FieldEdited {
            id: id.clone(),
            index,
            value,
        })
    };
    let error = form.and_then(|f| f.errors.get(index)).cloned().flatten();
    match (&field.kind, value) {
        (FieldKind::Bool, FieldValue::Bool(on)) => checkbox(*on)
            .label(field.label.clone())
            .style(theme::check)
            .on_toggle(move |on| edit(FieldValue::Bool(on)))
            .into(),
        (FieldKind::Choice(options), FieldValue::Text(selected)) => column![
            small(state, field.label.clone()),
            pick_list(
                options.as_slice(),
                Some(selected.clone()),
                move |choice: String| { edit(FieldValue::Text(choice)) }
            )
            .width(Fill),
        ]
        .spacing(4)
        .into(),
        (_, FieldValue::Text(current)) => {
            let placeholder = field.placeholder.clone().unwrap_or_default();
            let mut column = column![
                small(state, field.label.clone()),
                text_input(&placeholder, current)
                    .style(theme::input)
                    .padding(6)
                    .on_input(move |text| edit(FieldValue::Text(text))),
            ]
            .spacing(4);
            if field.network {
                column = column.push(muted(
                    state,
                    "FerroLibri lets this plugin contact the host of this address, and its subdomains, only once you save it here.",
                ));
            }
            // D4: an address without TLS is allowed, but the user is told.
            if error.is_none() && field.kind == FieldKind::Url && is_plain_http(current) {
                column = column.push(
                    text("This address does not use https: the connection is not encrypted.")
                        .size(theme::font(state, 12.0))
                        .color(theme::DARK.warning),
                );
            }
            if let Some(error) = error {
                column = column.push(
                    text(error)
                        .size(theme::font(state, 12.0))
                        .color(theme::DARK.warning),
                );
            }
            column.into()
        }
        _ => Space::new().into(),
    }
}

// ---- Install dialog ------------------------------------------------------------------------

fn button_label<'a>(state: &State, label: impl Into<String>) -> iced::widget::Text<'a> {
    text(label.into()).size(theme::font(state, 13.0))
}

fn steps<'a>(state: &State, current: u8) -> Element<'a, Message> {
    let mut steps = row![].spacing(6).align_y(Alignment::Center);
    for (index, name) in ["Source", "Check", "Confirm"].into_iter().enumerate() {
        let number = index as u8 + 1;
        if number > 1 {
            steps = steps.push(muted(state, "→"));
        }
        let (label, color, font) = if number == current {
            (
                format!("{number}. {name}"),
                theme::DARK.text_primary,
                strong(),
            )
        } else if number < current {
            (format!("✓ {name}"), theme::DARK.ok, Font::DEFAULT)
        } else {
            (
                format!("{number}. {name}"),
                theme::DARK.text_muted,
                Font::DEFAULT,
            )
        };
        steps = steps.push(
            text(label)
                .size(theme::font(state, 12.0))
                .color(color)
                .font(font),
        );
    }
    steps.into()
}

fn footer<'a>(buttons: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut footer = row![Space::new().width(Fill)].spacing(8);
    for button in buttons {
        footer = footer.push(button);
    }
    container(footer).padding([16, 0]).into()
}

fn plain<'a>(state: &State, label: &'static str, message: Message) -> Element<'a, Message> {
    button(button_label(state, label))
        .style(theme::outline)
        .padding([6, 12])
        .on_press(message)
        .into()
}

fn primary<'a>(
    state: &State,
    label: impl Into<String>,
    message: Option<Message>,
) -> Element<'a, Message> {
    button(button_label(state, label))
        .style(theme::primary)
        .padding([6, 12])
        .on_press_maybe(message)
        .into()
}

pub fn install_dialog(state: &State) -> Element<'_, Message> {
    let Some(install) = state.plugins.install.as_ref() else {
        return Space::new().into();
    };
    let mut content = column![
        row![
            text("Install plugin")
                .size(theme::font(state, 16.0))
                .width(Fill),
            button(icons::view(icons::X))
                .padding(4)
                .style(theme::quiet)
                .on_press(msg(Msg::CloseInstall)),
        ]
        .align_y(Alignment::Center),
        steps(state, install.step),
    ]
    .spacing(14);
    content = content.push(if install.step == 1 {
        source_step(state, install)
    } else if let Some(busy) = &install.busy {
        busy_step(state, busy)
    } else if install.step == 2 {
        check_step(state, install)
    } else {
        confirm_step(state, install)
    });
    container(
        container(scrollable(content.padding(20)).direction(theme::vbar()))
            .width(Fill)
            .max_width(520)
            .style(theme::card),
    )
    .padding(24)
    .center_x(Fill)
    .into()
}

fn source_step<'a>(state: &'a State, install: &'a Install) -> Element<'a, Message> {
    let mode_button = |mode: Mode, icon: &'static [u8], label: &'static str| {
        let selected = install.mode == mode;
        button(
            row![icons::view(icon), button_label(state, label)]
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .padding([4, 10])
        .style(move |_, status| {
            theme::button_style(
                if selected {
                    theme::Button::Selected
                } else {
                    theme::Button::Quiet
                },
                status,
            )
        })
        .on_press(msg(Msg::SetMode(mode)))
    };
    let check = install.can_check().then(|| msg(Msg::Check));
    let mut body = column![
        row![
            mode_button(Mode::Folder, icons::FOLDER, "Folder"),
            mode_button(Mode::Address, icons::GLOBE, "Web address"),
        ]
        .spacing(4),
    ]
    .spacing(8);
    match install.mode {
        Mode::Folder => {
            body = body
                .push(small(state, "Plugin folder"))
                .push(
                    row![
                        text_input("D:\\Plugins\\standard-ebooks", &install.folder)
                            .style(theme::input)
                            .padding(8)
                            .on_input(|text| msg(Msg::FolderInput(text)))
                            .on_submit_maybe(check.clone()),
                        button(button_label(state, "Browse…"))
                            .style(theme::outline)
                            .padding(8)
                            .on_press(msg(Msg::Browse)),
                    ]
                    .spacing(8),
                )
                .push(small(
                    state,
                    "The folder must contain plugin.toml and the plugin file. Nothing is copied or run until you confirm.",
                ));
        }
        Mode::Address => {
            body = body
                .push(small(state, "Web address"))
                .push(
                    text_input("https://github.com/owner/repo", &install.address)
                        .style(theme::input)
                        .padding(8)
                        .on_input(|text| msg(Msg::AddressInput(text)))
                        .on_submit_maybe(check.clone()),
                )
                .push(small(
                    state,
                    "A GitHub repository (its latest releases are listed) or a direct link to plugin.toml. Nothing is installed until you confirm.",
                ));
        }
    }
    body.push(footer(vec![
        plain(state, "Cancel", msg(Msg::CloseInstall)),
        primary(state, "Check", check),
    ]))
    .into()
}

fn busy_step<'a>(state: &State, busy: &str) -> Element<'a, Message> {
    column![
        container(
            column![
                icons::view(icons::REFRESH),
                text(busy.to_owned())
                    .size(theme::font(state, 13.0))
                    .color(theme::DARK.text_secondary),
            ]
            .spacing(12)
            .align_x(Alignment::Center),
        )
        .padding([26, 10])
        .center_x(Fill),
        footer(vec![plain(state, "Cancel", msg(Msg::AbortBusy))]),
    ]
    .into()
}

fn problem_card<'a>(state: &State, problem: &Problem) -> Element<'a, Message> {
    card(
        column![
            text(problem.title.clone())
                .size(theme::font(state, 14.0))
                .font(strong()),
            small(state, problem.message.clone()),
        ]
        .spacing(8),
    )
}

fn check_step<'a>(state: &'a State, install: &'a Install) -> Element<'a, Message> {
    if let Some(problem) = &install.problem {
        let mut buttons = Vec::new();
        if problem.kind == ProblemKind::Checksum && install.releases.is_some() {
            buttons.push(primary(
                state,
                "Choose another release",
                Some(msg(Msg::ChooseAnotherRelease)),
            ));
        } else {
            buttons.push(plain(state, "Back", msg(Msg::Back)));
            if problem.kind != ProblemKind::RateLimited {
                buttons.push(primary(state, "Try again", Some(msg(Msg::Check))));
            }
        }
        return column![problem_card(state, problem), footer(buttons)].into();
    }
    if let Some(candidate) = &install.candidate {
        let mut body = column![verdict(state, candidate, false)].spacing(12);
        if !candidate.reasons.is_empty() {
            let mut reasons = column![
                text("This plugin cannot be installed")
                    .size(theme::font(state, 14.0))
                    .font(strong())
            ]
            .spacing(4);
            for reason in &candidate.reasons {
                reasons = reasons.push(bullet(state, reason.clone()));
            }
            reasons = reasons.push(small(
                state,
                "Look for a release that supports plugin API 1, or ask the author for one.",
            ));
            body = body.push(card(reasons));
        }
        let mut buttons = vec![plain(state, "Back", msg(Msg::Back))];
        if candidate.compatible {
            buttons.push(primary(state, "Continue", Some(msg(Msg::Continue))));
        }
        return body.push(footer(buttons)).into();
    }
    if let Some(releases) = &install.releases {
        return release_list(state, releases);
    }
    column![footer(vec![plain(state, "Back", msg(Msg::Back))])].into()
}

fn verdict<'a>(state: &State, candidate: &Candidate, confirm: bool) -> Element<'a, Message> {
    let mut head = row![
        text(candidate.name.clone())
            .size(theme::font(state, 14.0))
            .font(strong()),
        badge(
            state,
            format!("v{}", candidate.version),
            theme::DARK.text_secondary
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let mut body = column![].spacing(8);
    if confirm {
        head = head.push(small(state, format!("by {}", candidate.author)));
        body = body.push(head);
        let note = match &candidate.change {
            Change::New => None,
            Change::Upgrade { from, .. } => Some(format!("Replaces your v{from}.")),
            Change::Downgrade { from, .. } => Some(format!(
                "Replaces your newer v{from} with an older version."
            )),
            Change::Reinstall { version } => {
                Some(format!("Same version as the one installed (v{version})."))
            }
        };
        if let Some(note) = note {
            body = body.push(small(state, note));
        }
    } else {
        head = head.push(if candidate.compatible {
            badge(state, "Compatible", theme::DARK.ok)
        } else {
            badge(state, "Not compatible", theme::DARK.error)
        });
        body = body.push(head);
        let meta = |label: &'static str, value: String| -> Element<'a, Message> {
            row![
                muted(state, label).width(78),
                small(state, value).width(Fill)
            ]
            .spacing(8)
            .into()
        };
        body = body
            .push(meta("Author", candidate.author.clone()))
            .push(meta("License", candidate.license.clone()))
            .push(meta("Homepage", candidate.homepage.clone()))
            .push(meta("Source", candidate.source.clone()))
            .push(meta("Plugin API", candidate.api.clone()));
        if candidate.checksum_in_manifest {
            body = body.push(meta(
                "Checksum",
                "SHA-256 matches the value in plugin.toml".into(),
            ));
        }
    }
    card(body)
}

fn release_list<'a>(state: &'a State, releases: &'a Releases) -> Element<'a, Message> {
    let mut body = column![
        row![
            small(state, "Releases of"),
            text(releases.repo.clone())
                .size(theme::font(state, 12.0))
                .font(strong()),
            small(
                state,
                format!("— the latest {RELEASES_SHOWN}, pre-releases hidden. Choose one to check."),
            ),
        ]
        .spacing(4)
        .wrap(),
    ]
    .spacing(8);
    if releases.items.is_empty() {
        body = body.push(card(
            column![
                text("No plugin found")
                    .size(theme::font(state, 14.0))
                    .font(strong()),
                small(
                    state,
                    "This repository has no published releases, or the address is not a plugin. FerroLibri installs from releases, not from a branch.",
                ),
            ]
            .spacing(8),
        ));
    } else if !releases.items.iter().any(|r| r.compatible) {
        body = body.push(card(
            column![
                text("No compatible release")
                    .size(theme::font(state, 14.0))
                    .font(strong()),
                small(
                    state,
                    "None of the latest releases works with this FerroLibri.",
                ),
            ]
            .spacing(8),
        ));
    }
    for release in &releases.items {
        let mut line = column![
            row![
                text(release.tag.clone())
                    .size(theme::font(state, 13.0))
                    .font(strong()),
                muted(state, release.date.clone()).width(Fill),
                if release.compatible {
                    badge(state, "Compatible", theme::DARK.ok)
                } else {
                    badge(state, "Not compatible", theme::DARK.error)
                },
            ]
            .spacing(10)
            .align_y(Alignment::Center)
        ]
        .spacing(2);
        if !release.compatible && !release.reason.is_empty() {
            line = line.push(muted(state, release.reason.clone()));
        }
        body = body.push(
            button(line)
                .width(Fill)
                .padding([8, 10])
                .style(theme::outline)
                .on_press_maybe(
                    release
                        .compatible
                        .then(|| msg(Msg::PickRelease(release.tag.clone()))),
                ),
        );
    }
    body.push(footer(vec![plain(state, "Back", msg(Msg::Back))]))
        .into()
}

fn confirm_step<'a>(state: &'a State, install: &'a Install) -> Element<'a, Message> {
    let Some(candidate) = &install.candidate else {
        return column![footer(vec![plain(state, "Back", msg(Msg::Back))])].into();
    };
    let mut body = column![
        verdict(state, candidate, true),
        card(permissions(state, &candidate.permissions)),
    ]
    .spacing(12);
    if matches!(candidate.change, Change::Upgrade { .. }) {
        body = body.push(small(
            state,
            "Settings you saved are kept. If the new version adds an address setting that can reach the network, enter it again.",
        ));
    }
    let downgrade = matches!(candidate.change, Change::Downgrade { .. });
    if let Change::Downgrade { from, to } = &candidate.change {
        body = body.push(
            checkbox(install.allow_downgrade)
                .label(format!(
                    "I want to go back to v{to} even though v{from} is installed"
                ))
                .style(theme::check)
                .on_toggle(|on| msg(Msg::AllowDowngrade(on))),
        );
    }
    if candidate.change == Change::New {
        body = body.push(
            checkbox(install.turn_on)
                .label("Turn on after installing")
                .style(theme::check)
                .on_toggle(|on| msg(Msg::TurnOn(on))),
        );
    }
    let label = match &candidate.change {
        Change::New => "Install".to_owned(),
        Change::Upgrade { to, .. } => format!("Update to v{to}"),
        Change::Downgrade { .. } => "Install older version".to_owned(),
        Change::Reinstall { version } => format!("Reinstall v{version}"),
    };
    let enabled = !downgrade || install.allow_downgrade;
    body.push(footer(vec![
        plain(state, "Back", msg(Msg::Back)),
        primary(state, label, enabled.then(|| msg(Msg::Install))),
    ]))
    .into()
}
