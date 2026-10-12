use crate::{
    keymap,
    response_guard::{GenerationGuard, RequestKey, ResponseGuard},
    services::{
        Services,
        jobs::{JobId, JobView},
    },
    settings::UiSettings,
    theme,
};
use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Fill, Subscription, Task};
use rulibre_core::{
    book::{Book, BookId, Format},
    cover::CoverFile,
    jobs::{JobOutput, JobRequest, JobState},
    metadata::Metadata,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Format(Format),
    Author(String),
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Setup,
    Home {
        filter: Filter,
    },
    Search,
    Jobs,
    Settings,
}

#[derive(Clone, Copy, Debug)]
pub enum FacetGroup {
    Library,
    Formats,
    Authors,
}
pub struct SidebarExpansion {
    pub library: bool,
    pub formats: bool,
    pub authors: bool,
}
impl Default for SidebarExpansion {
    fn default() -> Self {
        Self {
            library: true,
            formats: false,
            authors: false,
        }
    }
}

#[derive(Default)]
pub struct State {
    pub readers: crate::services::devices::ReaderState,
    pub reader_picker: crate::services::devices::ReaderPicker,
    pub screen: Screen,
    pub settings: UiSettings,
    pub notices: Vec<String>,
    pub jobs: BTreeMap<JobId, JobView>,
    pub results: BTreeMap<JobId, Arc<Result<JobOutput, String>>>,
    pub job_messages: BTreeMap<JobId, String>,
    pub finished_jobs: std::collections::VecDeque<JobId>,
    pub send_readers: BTreeMap<JobId, String>,
    pub send_devices: BTreeMap<JobId, rulibre_core::device::DeviceId>,
    pub toasts: Vec<crate::widgets::toasts::Toast>,
    pub next_toast: u64,
    pub metadata: Option<Metadata>,
    pub metadata_error: Option<String>,
    pub backends: Vec<rulibre_core::converter::Backend>,
    pub backend_job: Option<JobId>,
    pub backend_error: Option<String>,
    pub convert_menu: bool,
    pub pending_conversion: Option<crate::widgets::details::ConversionBatch>,
    pub conversion_export: Option<PathBuf>,
    pub export_prompt: bool,
    pub cover: Option<CoverFile>,
    pub picked_folder: Option<PathBuf>,
    pub metadata_guard: ResponseGuard,
    pub cover_guard: ResponseGuard,
    pub library: Option<PathBuf>,
    pub books: Vec<Book>,
    pub catalog: crate::catalog::Catalog,
    pub selection: crate::selection::Selection,
    pub modifiers: iced::keyboard::Modifiers,
    pub list_offset: f32,
    pub list_height: f32,
    pub show_warnings: bool,
    pub covers: crate::services::covers::CoverCache,
    pub viewport_width: f32,
    pub window_width: f32,
    pub pointer_x: f32,
    pub details_drag: Option<(f32, f32)>,
    pub dragged_details_width: Option<f32>,
    cover_errors: std::collections::BTreeSet<BookId>,
    pub sidebar_expansion: SidebarExpansion,
    pub facet_order: Vec<BookId>,
    pub search_query: String,
    /// The last selection change came from the keyboard: draw the cursor outline.
    pub keyboard_focus: bool,
    /// Where Esc returns from Search.
    pub search_return: Option<Screen>,
    pub search_order: Vec<BookId>,
    pub search_guard: GenerationGuard,
    pub search_running: Option<u64>,
    search_dirty: bool,
    pub search_error: Option<String>,
    pub scan_warnings: Vec<String>,
    pub scan_status: ScanStatus,
    pub setup_path: String,
    pub setup_error: Option<String>,
    pub library_loading: bool,
    pub setup_busy: bool,
    setup_native_path: Option<PathBuf>,
    library_generation: u64,
    library_saving: bool,
    settings_library_change: bool,
    pub export_busy: bool,
    export_generation: u64,
    pub active_scan: Option<JobId>,
    metadata_requests: BTreeMap<JobId, RequestKey>,
    settings_loading: bool,
    settings_saving: bool,
    pending_settings: Option<UiSettings>,
    requested_settings: Option<UiSettings>,
    pub closing: bool,
    /// Scrollbars are shown (recent scrolling); hidden after the idle delay.
    pub scroll_visible: bool,
    scroll_last: Option<std::time::Instant>,
    pub blur: crate::blur::BlurState,
}
impl State {
    pub fn row_height(&self) -> f32 {
        theme::font(self, crate::catalog::ROW_HEIGHT)
    }
    pub fn details_width(&self) -> f32 {
        let window = if self.window_width > 0.0 {
            self.window_width
        } else {
            1100.0
        };
        let sidebar = if self.visible_settings().sidebar_visible {
            theme::SIDEBAR_WIDTH
        } else {
            0.0
        };
        let limit = (window
            - sidebar
            - (self.visible_settings().cover_size as f32 + 36.0).max(180.0)
            - theme::DETAILS_GRIP)
            .clamp(240.0, 480.0);
        self.dragged_details_width
            .unwrap_or(self.visible_settings().details_width as f32)
            .clamp(240.0, limit)
    }
    pub fn visible_order(&self) -> &[BookId] {
        order_of(
            &self.screen,
            &self.search_order,
            &self.catalog,
            &self.facet_order,
        )
    }
    pub fn visible_settings(&self) -> &UiSettings {
        self.requested_settings.as_ref().unwrap_or(&self.settings)
    }
}
/// The books shown on `screen`, borrowed field by field so callers can keep
/// mutating `selection` without copying a 20,000-entry order.
pub fn order_of<'a>(
    screen: &Screen,
    search: &'a [BookId],
    catalog: &'a crate::catalog::Catalog,
    facets: &'a [BookId],
) -> &'a [BookId] {
    match screen {
        Screen::Search => search,
        Screen::Home {
            filter: Filter::All,
        } => &catalog.order,
        _ => facets,
    }
}
#[derive(Default, Debug, PartialEq, Eq)]
pub enum ScanStatus {
    #[default]
    Idle,
    Running,
    Ready,
    Cancelled,
    Failed(String),
}
#[derive(Debug, Clone)]
pub enum Message {
    Devices(crate::services::devices::DeviceUpdate),
    ToggleReaderMenu,
    CloseReaderMenu,
    ReaderMenuInside,
    SelectReader(rulibre_core::device::DeviceId),
    ManualReader,
    ReaderKind(rulibre_core::device::DeviceKind),
    ReaderPath(String),
    ReaderBrowse,
    ReaderFolderPicked {
        generation: u64,
        result: Result<Option<PathBuf>, String>,
    },
    ValidateReader,
    ReaderValidated {
        generation: u64,
        result: Result<rulibre_core::device::Device, String>,
    },
    Send,
    RefreshBackends,
    ToggleConvert,
    Ignore,
    Convert(Format),
    CancelConvert,
    UseExport,
    OpenFolder(PathBuf),
    Clock(std::time::Instant),
    ScrollActivity,
    DismissToast(u64),
    DismissNotice(usize),
    Viewport(iced::Size),
    ThumbnailLoaded {
        id: BookId,
        generation: u64,
        result: Result<Option<iced::widget::image::Handle>, String>,
    },
    LoadLibrary,
    LibraryLoaded {
        generation: u64,
        result: Result<Option<PathBuf>, String>,
    },
    SetupPathChanged(String),
    SetupBrowse,
    SetupFolderPicked {
        generation: u64,
        result: Result<Option<PathBuf>, String>,
    },
    ContinueSetup,
    LibraryValidated {
        generation: u64,
        result: Result<PathBuf, String>,
    },
    LibrarySaved {
        generation: u64,
        result: Result<PathBuf, String>,
    },
    ChangeLibrary,
    SettingsLibraryBrowse,
    SettingsLibraryPicked {
        generation: u64,
        result: Result<Option<PathBuf>, String>,
    },
    SetTextSize(u16),
    SetCoverSize(u16),
    StartDetailsResize,
    FinishDetailsResize,
    PointerMoved(f32),
    WindowResized(iced::Size),
    SetWindowOpacity(u8),
    SetWindowBlur(bool),
    WindowDrag,
    WindowMinimize,
    WindowToggleMaximize,
    WindowClose,
    WindowResize(iced::window::Direction),
    BlurApplied {
        on: bool,
        result: Result<(), String>,
    },
    SetPreferKepub(bool),
    ExportBrowse,
    ExportPicked {
        generation: u64,
        result: Result<Option<PathBuf>, String>,
    },
    ExportValidated {
        generation: u64,
        result: Result<PathBuf, String>,
    },
    Rescan,
    LoadSettings,
    SettingsLoaded(Result<UiSettings, String>),
    SaveSettings(UiSettings),
    SettingsSaved {
        settings: UiSettings,
        result: Result<(), String>,
    },
    ToggleSidebar,
    ToggleDetails,
    Navigate(Screen),
    ToggleFacetGroup(FacetGroup),
    SearchChanged(String),
    SearchSubmit,
    SearchFinished {
        generation: u64,
        result: Result<Vec<BookId>, String>,
    },
    Submit {
        title: String,
        request: JobRequest,
    },
    Tick,
    Cancel(JobId),
    ClearFinished,
    LoadMetadata {
        book: BookId,
        path: PathBuf,
    },
    LoadCover {
        book: BookId,
        path: PathBuf,
    },
    CoverLoaded {
        key: RequestKey,
        result: Result<Option<CoverFile>, String>,
    },
    ClearSelection,
    BookClicked(BookId),
    Keyboard(iced::keyboard::Event),
    ResetModifiers,
    ListScrolled {
        offset: f32,
        height: f32,
    },
    ShowWarnings,
    ShowList,
    ShowGrid,
    PickFolder,
    FolderPicked(Result<Option<PathBuf>, String>),
    Close(iced::window::Id),
}

pub fn update(state: &mut State, message: Message, services: &dyn Services) -> Task<Message> {
    let task = update_inner(state, message, services);
    Task::batch([task, sync_blur(state)])
}
/// Apply or clear the OS backdrop when blur is on and the window is translucent.
fn sync_blur(state: &mut State) -> Task<Message> {
    let wanted = state.blur.ready
        && !state.blur.failed
        && !state.closing
        && state.visible_settings().window_blur
        && state.visible_settings().window_opacity < 100;
    if state.blur.pending || state.blur.applied == wanted {
        return Task::none();
    }
    state.blur.pending = true;
    iced::window::oldest().then(move |id| match id {
        Some(id) => iced::window::run(id, move |handle| crate::blur::apply(handle, wanted))
            .map(move |result| Message::BlurApplied { on: wanted, result }),
        None => Task::done(Message::BlurApplied {
            on: wanted,
            result: Err("No window".into()),
        }),
    })
}
/// Strength used by fills: 0 until the OS backdrop is really in place.
pub fn blur_alpha(state: &State) -> f32 {
    let settings = state.visible_settings();
    let active = state.blur.ready
        && !state.blur.failed
        && settings.window_opacity < 100
        && (!settings.window_blur || state.blur.applied);
    crate::blur::fill_alpha(settings.window_opacity, active)
}
/// Frame clear colour: transparent only while the blur is shown.
pub fn style(state: &State, _: &iced::Theme) -> iced::theme::Style {
    iced::theme::Style {
        background_color: if blur_alpha(state) < 1.0 {
            iced::Color::TRANSPARENT
        } else {
            theme::DARK.background
        },
        text_color: theme::DARK.text_primary,
    }
}
fn update_inner(state: &mut State, message: Message, services: &dyn Services) -> Task<Message> {
    match message {
        Message::WindowResized(size) => state.window_width = size.width,
        Message::PointerMoved(x) => {
            state.pointer_x = x;
            if let Some((start, width)) = state.details_drag {
                state.dragged_details_width = Some(width + start - x);
            }
        }
        Message::StartDetailsResize
            if matches!(state.screen, Screen::Home { .. }) && !state.closing =>
        {
            state.details_drag = Some((state.pointer_x, state.details_width()));
        }
        Message::StartDetailsResize => {}
        Message::FinishDetailsResize => {
            if state.details_drag.take().is_some() {
                let width = state.details_width().round() as u16;
                state.dragged_details_width = None;
                let mut settings = state.visible_settings().clone();
                settings.details_width = width;
                return update(state, Message::SaveSettings(settings), services);
            }
        }
        Message::SetTextSize(size) | Message::SetCoverSize(size) if !state.closing => {
            let mut settings = state.visible_settings().clone();
            if matches!(message, Message::SetTextSize(_)) {
                settings.text_size = size;
            } else {
                settings.cover_size = size;
            }
            settings.normalize();
            state.list_offset = 0.0;
            return Task::batch([
                update(state, Message::SaveSettings(settings), services),
                reset_list_scroll(),
            ]);
        }
        Message::SetTextSize(_) | Message::SetCoverSize(_) => {}

        Message::OpenFolder(path) => {
            return Task::perform(services.open_folder(path), |result| match result {
                Ok(()) => Message::Tick,
                Err(error) => Message::FolderPicked(Err(error)),
            });
        }
        Message::RefreshBackends if !state.closing && state.backend_job.is_none() => {
            state.backend_error = None;
            state.backend_job = Some(services.list_backends());
            poll(state, services);
        }
        Message::RefreshBackends => {}
        Message::Ignore => {}
        Message::ToggleConvert => state.convert_menu = !state.convert_menu,
        Message::CancelConvert => {
            state.pending_conversion = None;
            state.export_prompt = false;
            state.conversion_export = None;
            state.export_generation += 1;
            state.export_busy = false;
        }
        Message::UseExport if state.export_prompt && !state.export_busy => {
            if let Some(path) = state.conversion_export.clone() {
                let mut settings = state.visible_settings().clone();
                settings.export_folder = Some(path);
                state.export_prompt = false;
                return update(state, Message::SaveSettings(settings), services);
            }
        }
        Message::UseExport => {}
        Message::Convert(target)
            if !state.closing
                && matches!(state.screen, Screen::Home { .. })
                && state.pending_conversion.is_none()
                && !state.export_busy
                && !state.setup_busy =>
        {
            if let Some(batch) = crate::widgets::details::batch(state, target) {
                state.convert_menu = false;
                if let Some(output) = state.settings.export_folder.clone() {
                    batch.enqueue(services, output);
                    poll(state, services);
                } else {
                    state.pending_conversion = Some(batch);
                    state.export_prompt = true;
                }
            }
        }
        Message::Convert(_) => {}
        Message::BlurApplied { on, result } => {
            state.blur.pending = false;
            match result {
                Ok(()) => state.blur.applied = on,
                Err(error) => {
                    state.blur.failed = true;
                    log::warn!("Window blur unavailable: {error}");
                    crate::widgets::toasts::push(
                        state,
                        format!("Window blur is not available on this system. {error}"),
                        false,
                    );
                }
            }
        }
        Message::WindowDrag if !state.closing => {
            return iced::window::oldest().then(|id| match id {
                Some(id) => iced::window::drag(id),
                None => Task::none(),
            });
        }
        Message::WindowMinimize if !state.closing => {
            return iced::window::oldest().then(|id| match id {
                Some(id) => iced::window::minimize(id, true),
                None => Task::none(),
            });
        }
        Message::WindowToggleMaximize if !state.closing => {
            return iced::window::oldest().then(|id| match id {
                Some(id) => iced::window::toggle_maximize(id),
                None => Task::none(),
            });
        }
        Message::WindowClose if !state.closing => {
            return iced::window::oldest().then(|id| match id {
                Some(id) => Task::done(Message::Close(id)),
                None => Task::none(),
            });
        }
        Message::WindowResize(direction) if !state.closing => {
            return iced::window::oldest().then(move |id| match id {
                Some(id) => iced::window::drag_resize(id, direction),
                None => Task::none(),
            });
        }
        Message::WindowDrag
        | Message::WindowMinimize
        | Message::WindowToggleMaximize
        | Message::WindowClose
        | Message::WindowResize(_) => {}
        Message::SetWindowOpacity(value) if !state.closing => {
            let mut settings = state.visible_settings().clone();
            settings.window_opacity = value;
            return update(state, Message::SaveSettings(settings), services);
        }
        Message::SetWindowBlur(value) if !state.closing => {
            let mut settings = state.visible_settings().clone();
            settings.window_blur = value;
            return update(state, Message::SaveSettings(settings), services);
        }
        Message::SetPreferKepub(value) if !state.closing => {
            let mut settings = state.visible_settings().clone();
            settings.prefer_kepub = value;
            return update(state, Message::SaveSettings(settings), services);
        }
        Message::SettingsLibraryBrowse
            if !state.setup_busy
                && !state.library_loading
                && !state.closing
                && !state.export_busy =>
        {
            state.setup_busy = true;
            state.settings_library_change = true;
            let generation = next_library_generation(state);
            return Task::perform(services.pick_folder(), move |result| {
                Message::SettingsLibraryPicked { generation, result }
            });
        }
        Message::SettingsLibraryPicked { generation, result }
            if generation == state.library_generation
                && state.settings_library_change
                && !state.closing =>
        {
            match result {
                Ok(Some(path)) => {
                    // save_library repeats the same core Setup validation using
                    // the native path, without a lossy display-string round trip.
                    return update(
                        state,
                        Message::LibraryValidated {
                            generation,
                            result: Ok(path),
                        },
                        services,
                    );
                }
                Ok(None) => {
                    state.setup_busy = false;
                    state.settings_library_change = false;
                }
                Err(error) => settings_library_error(state, error),
            }
        }
        Message::ExportBrowse
            if !state.export_busy
                && !state.setup_busy
                && !state.library_loading
                && !state.closing
                && state.library.is_some() =>
        {
            state.export_busy = true;
            state.export_generation += 1;
            let generation = state.export_generation;
            return Task::perform(services.pick_folder(), move |result| {
                Message::ExportPicked { generation, result }
            });
        }
        Message::ExportPicked { generation, result }
            if generation == state.export_generation && state.export_busy && !state.closing =>
        {
            match result {
                Ok(Some(path)) => {
                    return Task::perform(
                        services.validate_export(
                            state.library.clone().expect("configured library"),
                            path,
                        ),
                        move |result| Message::ExportValidated { generation, result },
                    );
                }
                Ok(None) => state.export_busy = false,
                Err(error) => {
                    state.export_busy = false;
                    crate::widgets::toasts::push(state, error, true);
                }
            }
        }
        Message::ExportValidated { generation, result }
            if generation == state.export_generation && state.export_busy && !state.closing =>
        {
            state.export_busy = false;
            match result {
                Ok(path) => {
                    let mut settings = state.visible_settings().clone();
                    if state.export_prompt {
                        state.conversion_export = Some(path);
                    } else {
                        settings.export_folder = Some(path);
                        return update(state, Message::SaveSettings(settings), services);
                    }
                }
                Err(error) => crate::widgets::toasts::push(state, error, true),
            }
        }
        Message::SetWindowOpacity(_)
        | Message::SetWindowBlur(_)
        | Message::SetPreferKepub(_)
        | Message::SettingsLibraryBrowse
        | Message::SettingsLibraryPicked { .. }
        | Message::ExportBrowse
        | Message::ExportPicked { .. }
        | Message::ExportValidated { .. } => {}
        Message::Devices(update) if !state.closing => state.readers.apply(update),
        Message::Devices(_) => {}
        Message::ToggleReaderMenu if !state.closing => {
            if state.reader_picker.open {
                state.reader_picker.close();
            } else {
                state.reader_picker.open = true;
                state.reader_picker.manual = false;
            }
        }
        Message::CloseReaderMenu => state.reader_picker.close(),
        Message::ReaderMenuInside => {}
        Message::SelectReader(id) if !state.closing => {
            if state.readers.select(id) {
                state.reader_picker.close();
            }
        }
        Message::ManualReader if !state.closing => {
            state.reader_picker.open = true;
            state.reader_picker.manual = true;
            state.reader_picker.error = None;
        }
        Message::ReaderKind(kind) if !state.reader_picker.busy => {
            state.reader_picker.kind = kind;
            state.reader_picker.error = None;
        }
        Message::ReaderPath(path) if !state.reader_picker.busy => {
            state.reader_picker.path = path;
            state.reader_picker.native_path = None;
            state.reader_picker.error = None;
        }
        Message::ReaderBrowse
            if state.reader_picker.open && !state.reader_picker.busy && !state.closing =>
        {
            state.reader_picker.busy = true;
            state.reader_picker.generation += 1;
            let generation = state.reader_picker.generation;
            return Task::perform(services.pick_folder(), move |result| {
                Message::ReaderFolderPicked { generation, result }
            });
        }
        Message::ReaderFolderPicked { generation, result }
            if state.reader_picker.open
                && generation == state.reader_picker.generation
                && !state.closing =>
        {
            state.reader_picker.busy = false;
            match result {
                Ok(Some(path)) => {
                    state.reader_picker.path = path.display().to_string();
                    state.reader_picker.native_path = Some(path);
                    state.reader_picker.error = None;
                }
                Ok(None) => {}
                Err(error) => state.reader_picker.error = Some(error),
            }
        }
        Message::ValidateReader
            if state.reader_picker.open && !state.reader_picker.busy && !state.closing =>
        {
            let path = match state
                .reader_picker
                .native_path
                .clone()
                .map(Ok)
                .unwrap_or_else(|| {
                    rulibre_core::config::sanitize_path(&state.reader_picker.path)
                        .map_err(|e| e.to_string())
                }) {
                Ok(path) => path,
                Err(error) => {
                    state.reader_picker.error = Some(error);
                    return Task::none();
                }
            };
            state.reader_picker.busy = true;
            state.reader_picker.generation += 1;
            let generation = state.reader_picker.generation;
            return Task::perform(
                services.validate_reader(path, state.reader_picker.kind),
                move |result| Message::ReaderValidated { generation, result },
            );
        }
        Message::ReaderValidated { generation, result }
            if state.reader_picker.open
                && generation == state.reader_picker.generation
                && !state.closing =>
        {
            state.reader_picker.busy = false;
            match result {
                Ok(reader) => {
                    state.readers.revision = services.remember_reader(&reader);
                    state
                        .readers
                        .devices
                        .retain(|d| d.mount_point != reader.mount_point);
                    state.readers.selected_id = Some(reader.id.clone());
                    state.readers.devices.push(reader);
                    state.readers.devices.sort_by(|a, b| a.id.cmp(&b.id));
                    state.readers.failure = None;
                    state.reader_picker.close();
                }
                Err(error) => state.reader_picker.error = Some(error),
            }
        }
        Message::Send => {
            if state.closing
                || state.library_loading
                || state.screen == Screen::Setup
                || !state.readers.can_send(state.selection.selected.len())
            {
                return Task::none();
            }
            let reader = state.readers.selected().unwrap().clone();
            let options = rulibre_core::device::PreparationOptions {
                kobo: if state.settings.prefer_kepub {
                    rulibre_core::device::KoboPreparation::PreferKepub
                } else {
                    rulibre_core::device::KoboPreparation::Compatible
                },
            };
            for id in state.selection.ordered() {
                if let Some(entry) = state.catalog.get(&id) {
                    let id = services.send(
                        entry.book.title.clone(),
                        entry.book.path.clone(),
                        entry.book.author.clone(),
                        reader.clone(),
                        options,
                    );
                    state.send_readers.insert(id, reader.name().to_owned());
                    state.send_devices.insert(id, reader.id.clone());
                    // Reflect submission immediately, even if a service only
                    // reports running jobs on its next event-loop poll.
                    state.jobs.insert(
                        id,
                        JobView {
                            id,
                            title: entry.book.title.clone(),
                            kind: crate::services::jobs::JobKind::Send,
                            state: JobState::Waiting,
                            stage: None,
                            progress: None,
                            error: None,
                        },
                    );
                }
            }
            poll(state, services);
        }
        Message::ScrollActivity => {
            state.scroll_visible = true;
            state.scroll_last = Some(std::time::Instant::now());
        }
        Message::Clock(now) => {
            if let Some(last) = state.scroll_last
                && now.saturating_duration_since(last)
                    >= Duration::from_secs(theme::SCROLLBAR_IDLE_SECS)
            {
                state.scroll_visible = false;
                state.scroll_last = None;
            }
            crate::widgets::toasts::advance(state, now);
            return update(state, Message::Tick, services);
        }
        Message::DismissToast(id) => state.toasts.retain(|t| t.id != id),
        Message::DismissNotice(index) => {
            if index < state.notices.len() {
                state.notices.remove(index);
            }
        }
        Message::ToggleReaderMenu
        | Message::SelectReader(_)
        | Message::ManualReader
        | Message::ReaderKind(_)
        | Message::ReaderPath(_)
        | Message::ReaderBrowse
        | Message::ReaderFolderPicked { .. }
        | Message::ValidateReader
        | Message::ReaderValidated { .. } => {}
        Message::Viewport(size) => {
            state.viewport_width = size.width;
            state.list_height = size.height;
            return load_visible_covers(state, services);
        }
        Message::ThumbnailLoaded {
            id,
            generation,
            result,
        } => {
            let pending = load_visible_covers(state, services);
            let error = result.as_ref().err().cloned();
            if state
                .covers
                .finish(id.clone(), generation, result.unwrap_or(None))
                && let Some(error) = error
                && state.cover_errors.insert(id.clone())
            {
                log::warn!("Could not load cover {id:?}: {error}");
            }
            return Task::batch([pending, load_visible_covers(state, services)]);
        }
        Message::LoadLibrary => {
            if state.library_loading || state.setup_busy {
                return Task::none();
            }
            state.library_loading = true;
            let generation = next_library_generation(state);
            return Task::perform(services.load_library(), move |result| {
                Message::LibraryLoaded { generation, result }
            });
        }
        Message::LibraryLoaded { generation, result }
            if generation == state.library_generation && !state.closing =>
        {
            state.library_loading = false;
            match result {
                Ok(Some(path)) => activate_library(state, path, services),
                Ok(None) => state.screen = Screen::Setup,
                Err(error) => {
                    state.screen = Screen::Setup;
                    state.setup_error = Some(error);
                }
            }
        }
        Message::SetupPathChanged(path) if !state.setup_busy && !state.library_loading => {
            state.setup_path = path;
            state.setup_native_path = None;
            state.setup_error = None;
        }
        Message::SetupBrowse if !state.setup_busy && !state.library_loading => {
            state.setup_busy = true;
            let generation = next_library_generation(state);
            return Task::perform(services.pick_folder(), move |result| {
                Message::SetupFolderPicked { generation, result }
            });
        }
        Message::SetupFolderPicked { generation, result }
            if generation == state.library_generation && !state.closing =>
        {
            state.setup_busy = false;
            match result {
                Ok(Some(path)) => {
                    state.setup_path = path.display().to_string();
                    state.setup_native_path = Some(path);
                    state.setup_error = None;
                }
                Ok(None) => {}
                Err(error) => state.setup_error = Some(error),
            }
        }
        Message::ContinueSetup
            if !state.setup_busy
                && !state.library_loading
                && !state.setup_path.trim().is_empty() =>
        {
            state.setup_busy = true;
            state.setup_error = None;
            let generation = next_library_generation(state);
            if let Some(path) = state.setup_native_path.clone() {
                return update(
                    state,
                    Message::LibraryValidated {
                        generation,
                        result: Ok(path),
                    },
                    services,
                );
            }
            return Task::perform(
                services.validate_library(state.setup_path.clone()),
                move |result| Message::LibraryValidated { generation, result },
            );
        }
        Message::LibraryValidated { generation, result }
            if generation == state.library_generation && !state.closing =>
        {
            match result {
                Ok(path) => {
                    state.library_saving = true;
                    return Task::perform(services.save_library(path), move |result| {
                        Message::LibrarySaved { generation, result }
                    });
                }
                Err(error) => {
                    if state.settings_library_change {
                        settings_library_error(state, error);
                    } else {
                        state.setup_busy = false;
                        state.setup_error = Some(error);
                    }
                }
            }
        }
        Message::LibrarySaved { generation, result }
            if generation == state.library_generation && !state.closing =>
        {
            state.library_saving = false;
            state.setup_busy = false;
            match result {
                Ok(path) => {
                    state.settings_library_change = false;
                    activate_library(state, path, services);
                }
                Err(error) if state.settings_library_change => settings_library_error(state, error),
                Err(error) => state.setup_error = Some(error),
            }
        }
        Message::ChangeLibrary if !state.library_saving => {
            state.settings_library_change = false;
            invalidate_search(state);
            next_library_generation(state);
            state.library_loading = false;
            state.setup_busy = false;
            cancel_scan(state, services);
            clear_selection(state);
            state.setup_path = state
                .library
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            state.setup_native_path = state.library.clone();
            state.setup_error = None;
            state.screen = Screen::Setup;
        }
        Message::Rescan if state.library.is_some() && !state.closing => start_scan(state, services),
        Message::LibraryLoaded { .. }
        | Message::SetupPathChanged(_)
        | Message::SetupBrowse
        | Message::SetupFolderPicked { .. }
        | Message::ContinueSetup
        | Message::LibraryValidated { .. }
        | Message::LibrarySaved { .. }
        | Message::ChangeLibrary
        | Message::Rescan => {}
        Message::LoadSettings => {
            state.settings_loading = true;
            return Task::perform(services.load_settings(), Message::SettingsLoaded);
        }
        Message::SettingsLoaded(result) => {
            state.settings_loading = false;
            match result {
                Ok(settings) => state.settings = settings,
                Err(error) => {
                    state.settings = UiSettings::default();
                    state.notices.push(error);
                }
            }
            if let Some(settings) = state.pending_settings.take() {
                return update(state, Message::SaveSettings(settings), services);
            }
        }
        Message::SaveSettings(mut settings) => {
            settings.normalize();
            state.requested_settings = Some(settings.clone());
            // Serialize writes, coalescing edits made during an outstanding save.
            // This also prevents an older write from winning after a newer one.
            if state.settings_saving || state.settings_loading {
                state.pending_settings = Some(settings);
            } else {
                state.settings_saving = true;
                let saved = settings.clone();
                return Task::perform(services.save_settings(settings), move |result| {
                    Message::SettingsSaved {
                        settings: saved.clone(),
                        result,
                    }
                });
            }
        }
        Message::SettingsSaved { settings, result } => {
            state.settings_saving = false;
            match result {
                Ok(()) => state.settings = settings,
                Err(error) => crate::widgets::toasts::push(
                    state,
                    format!("Could not save settings. Previous settings were kept. {error}"),
                    true,
                ),
            }
            if let Some(settings) = state.pending_settings.take() {
                return update(state, Message::SaveSettings(settings), services);
            }
            state.requested_settings = None;
            if !state.export_prompt
                && let Some(batch) = state.pending_conversion.take()
            {
                if state.settings.export_folder == state.conversion_export {
                    if let Some(output) = state.conversion_export.take() {
                        batch.enqueue(services, output);
                        poll(state, services);
                    }
                } else {
                    state.conversion_export = None;
                }
            }
        }
        Message::ToggleSidebar if state.screen != Screen::Setup && !state.library_loading => {
            let mut settings = state.visible_settings().clone();
            settings.sidebar_visible = !settings.sidebar_visible;
            return update(state, Message::SaveSettings(settings), services);
        }
        Message::ToggleDetails
            if matches!(state.screen, Screen::Home { .. }) && !state.library_loading =>
        {
            let mut settings = state.visible_settings().clone();
            settings.details_visible = !settings.details_visible;
            return update(state, Message::SaveSettings(settings), services);
        }
        Message::ToggleSidebar | Message::ToggleDetails => {}
        Message::Navigate(screen) => {
            if screen == Screen::Search && matches!(state.screen, Screen::Home { .. }) {
                state.search_return = Some(state.screen.clone());
            }
            if !matches!(screen, Screen::Home { .. }) {
                clear_selection(state);
            }
            state.screen = screen;
            state.list_offset = 0.0;
            state.show_warnings = false;
            refresh_filter(state);
            let scroll = reset_list_scroll();
            if state.screen == Screen::Search {
                return Task::batch([
                    scroll,
                    start_search(state),
                    iced::widget::operation::focus(search_id()),
                ]);
            }
            return scroll;
        }
        Message::ToggleFacetGroup(group) => {
            let value = match group {
                FacetGroup::Library => &mut state.sidebar_expansion.library,
                FacetGroup::Formats => &mut state.sidebar_expansion.formats,
                FacetGroup::Authors => &mut state.sidebar_expansion.authors,
            };
            *value = !*value;
        }
        Message::SearchSubmit => return run_command(state, keymap::Command::OpenDetails, services),
        Message::SearchChanged(query) => {
            state.search_query = query;
            invalidate_search(state);
            state.list_offset = 0.0;
            return Task::batch([start_search(state), reset_list_scroll()]);
        }
        Message::SearchFinished { generation, result } => {
            if state.search_running != Some(generation) {
                return Task::none();
            }
            state.search_running = None;
            if state.search_guard.accepts(generation) && !state.closing {
                match result {
                    Ok(order) => state.search_order = order,
                    Err(error) => state.search_error = Some(error),
                }
            }
            return start_search(state);
        }
        Message::Submit { title, request } => {
            services.enqueue(title, request);
            poll(state, services);
        }
        Message::Tick => {
            let scanning = state.active_scan.is_some();
            poll(state, services);
            if scanning && state.active_scan.is_none() && state.scan_status == ScanStatus::Ready {
                return Task::batch([
                    reset_list_scroll(),
                    start_search(state),
                    load_visible_covers(state, services),
                ]);
            }
            return load_visible_covers(state, services);
        }
        Message::Cancel(id) => {
            if state.active_scan == Some(id) {
                state.active_scan = None;
                state.scan_status = ScanStatus::Cancelled;
            }
            services.cancel(id);
            poll(state, services);
        }
        Message::ClearFinished => {
            services.clear_finished();
            state
                .jobs
                .retain(|_, job| matches!(job.state, JobState::Waiting | JobState::Running));
            state.results.retain(|id, _| state.jobs.contains_key(id));
            state
                .job_messages
                .retain(|id, _| state.jobs.contains_key(id));
            state
                .send_readers
                .retain(|id, _| state.jobs.contains_key(id));
            state.finished_jobs.clear();
            state
                .send_devices
                .retain(|id, _| state.jobs.contains_key(id));
        }
        Message::LoadMetadata { book, path } => {
            state.metadata = None;
            state.metadata_error = None;
            let key = state.metadata_guard.request(book);
            let id = services.read_metadata(path);
            state.metadata_requests.insert(id, key);
            poll(state, services);
        }
        Message::LoadCover { book, path } => {
            state.cover = None;
            let key = state.cover_guard.request(book);
            return Task::perform(services.find_cover(path), move |result| {
                Message::CoverLoaded {
                    key: key.clone(),
                    result,
                }
            });
        }
        Message::CoverLoaded { key, result } => {
            if state.cover_guard.accepts(&key) {
                match result {
                    Ok(cover) => state.cover = cover,
                    Err(error) => state.notices.push(error),
                }
            }
        }
        Message::ClearSelection => clear_selection(state),
        Message::BookClicked(id)
            if matches!(state.screen, Screen::Home { .. } | Screen::Search) =>
        {
            if !state.visible_order().contains(&id) {
                return Task::none();
            }
            let order = order_of(
                &state.screen,
                &state.search_order,
                &state.catalog,
                &state.facet_order,
            );
            state.selection.click(id, state.modifiers, order);
            state.keyboard_focus = false;
            if state.screen == Screen::Search {
                return Task::none();
            }
            return open_selected_details(state, services);
        }
        Message::BookClicked(_) => {}
        Message::ResetModifiers => {
            state.modifiers = iced::keyboard::Modifiers::empty();
            state.details_drag = None;
            state.dragged_details_width = None;
        }
        Message::Keyboard(event) => return handle_keyboard(state, event, services),
        Message::ListScrolled { offset, height } => {
            state.scroll_visible = true;
            state.scroll_last = Some(std::time::Instant::now());
            state.list_offset = offset;
            state.list_height = height;
            return load_visible_covers(state, services);
        }
        Message::ShowWarnings => state.show_warnings = !state.show_warnings,
        Message::ShowList | Message::ShowGrid => {
            let mut settings = state.visible_settings().clone();
            settings.view = if matches!(message, Message::ShowGrid) {
                crate::settings::ViewMode::Grid
            } else {
                crate::settings::ViewMode::List
            };
            state.list_offset = 0.0;
            let save = update(state, Message::SaveSettings(settings), services);
            return Task::batch([
                save,
                reset_list_scroll(),
                load_visible_covers(state, services),
            ]);
        }
        Message::PickFolder => return Task::perform(services.pick_folder(), Message::FolderPicked),
        Message::FolderPicked(result) => match result {
            Ok(path) => state.picked_folder = path,
            Err(error) => state.notices.push(error),
        },
        Message::Close(id) => {
            state.closing = true;
            state.pending_conversion = None;
            state.export_prompt = false;
            state.reader_picker.close();
            services.shutdown();
            return iced::window::close(id);
        }
    }
    Task::none()
}
fn next_library_generation(state: &mut State) -> u64 {
    state.library_generation = state
        .library_generation
        .checked_add(1)
        .expect("library generation exhausted");
    state.library_generation
}
fn load_visible_covers(state: &mut State, services: &dyn Services) -> Task<Message> {
    let mut wanted = if matches!(state.screen, Screen::Home { .. } | Screen::Search)
        && state.scan_status == ScanStatus::Ready
        && !state.show_warnings
        && !state.closing
    {
        let grid = state.screen != Screen::Search
            && state.visible_settings().view == crate::settings::ViewMode::Grid;
        let order = state.visible_order();
        let range = if grid {
            crate::widgets::book_grid::Geometry::for_state(state).book_range(
                order.len(),
                state.viewport_width,
                state.list_offset,
                state.list_height * 2.0,
            )
        } else {
            crate::catalog::visible_slice_with_height(
                order.len(),
                state.list_offset,
                state.list_height * 2.0,
                state.row_height(),
            )
        };
        order[range]
            .iter()
            .take(crate::services::covers::CAPACITY)
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    if matches!(state.screen, Screen::Home { .. }) && state.selection.selected.len() == 1 {
        let id = state.selection.selected.first().unwrap().clone();
        if !wanted.contains(&id) {
            wanted.insert(0, id);
        }
    }
    state.covers.request(wanted);
    let tasks = state
        .covers
        .next_requests()
        .into_iter()
        .filter_map(|(id, generation)| {
            let entry = state.catalog.get(&id)?;
            Some(Task::perform(
                services.load_thumbnail(id.clone(), entry.book.path.clone()),
                move |result| Message::ThumbnailLoaded {
                    id: id.clone(),
                    generation,
                    result,
                },
            ))
        });
    Task::batch(tasks)
}
pub fn search_id() -> iced::widget::Id {
    iced::widget::Id::new("search-query")
}
fn reset_list_scroll() -> Task<Message> {
    iced::widget::operation::scroll_to(
        crate::widgets::book_list::id(),
        iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
    )
}
fn refresh_filter(state: &mut State) {
    if let Screen::Home { ref filter } = state.screen {
        state.facet_order = state.catalog.filter(filter);
    }
}
fn invalidate_search(state: &mut State) {
    state.search_guard.invalidate();
    state.search_order.clear();
    state.search_error = None;
    state.search_dirty = !state.search_query.trim().is_empty();
}
fn start_search(state: &mut State) -> Task<Message> {
    if !state.search_dirty
        || state.search_running.is_some()
        || state.closing
        || state.scan_status != ScanStatus::Ready
    {
        return Task::none();
    }
    state.search_dirty = false;
    let generation = state.search_guard.current();
    state.search_running = Some(generation);
    let index = state.catalog.search_index.clone();
    let query = state.search_query.clone();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || index.search(&query))
                .await
                .map_err(|e| e.to_string())
        },
        move |result| Message::SearchFinished { generation, result },
    )
}
fn cancel_scan(state: &mut State, services: &dyn Services) {
    if let Some(id) = state.active_scan.take() {
        services.cancel(id);
    }
}
fn settings_library_error(state: &mut State, error: String) {
    state.setup_busy = false;
    state.settings_library_change = false;
    crate::widgets::toasts::push(state, error, true);
}
fn activate_library(state: &mut State, path: PathBuf, services: &dyn Services) {
    state.export_generation += 1;
    state.export_busy = false;
    state.pending_conversion = None;
    state.export_prompt = false;
    state.conversion_export = None;
    if state.library.as_ref() != Some(&path) {
        clear_selection(state);
        state.books.clear();
        state.catalog = crate::catalog::Catalog::default();
        state.facet_order.clear();
        invalidate_search(state);
    }
    state.library = Some(path);
    state.setup_error = None;
    state.screen = Screen::Home {
        filter: Filter::All,
    };
    start_scan(state, services);
}
fn start_scan(state: &mut State, services: &dyn Services) {
    state.covers.clear();
    state.cover_errors.clear();
    invalidate_search(state);
    cancel_scan(state, services);
    state.metadata_guard.clear();
    state.cover_guard.clear();
    state.metadata = None;
    state.metadata_error = None;
    state.cover = None;
    state.show_warnings = false;
    state.scan_warnings.clear();
    state.scan_status = ScanStatus::Running;
    state.active_scan = Some(services.scan(state.library.clone().expect("configured library")));
    poll(state, services);
}
fn clear_selection(state: &mut State) {
    state.selection.clear();
    state.metadata_error = None;
    state.convert_menu = false;
    state.metadata_guard.clear();
    state.cover_guard.clear();
    state.metadata = None;
    state.cover = None;
}
fn open_selected_details(state: &mut State, services: &dyn Services) -> Task<Message> {
    state.metadata_guard.clear();
    state.cover_guard.clear();
    state.metadata = None;
    state.metadata_error = None;
    state.cover = None;
    state.convert_menu = false;
    if state.selection.selected.len() == 1 {
        let id = state.selection.selected.first().unwrap().clone();
        if let Some(entry) = state.catalog.get(&id) {
            let path = entry.book.path.clone();
            let _ = update(state, Message::LoadMetadata { book: id, path }, services);
        }
    }
    let covers = load_visible_covers(state, services);
    if !state.visible_settings().details_visible {
        let mut settings = state.visible_settings().clone();
        settings.details_visible = true;
        return Task::batch([
            covers,
            update(state, Message::SaveSettings(settings), services),
        ]);
    }
    covers
}

fn key_context(state: &State) -> keymap::Context {
    use keymap::Pane;
    let pane = if state.library_loading || state.screen == Screen::Setup {
        Pane::Setup
    } else {
        match state.screen {
            Screen::Home { .. } => Pane::Library {
                grid: state.visible_settings().view == crate::settings::ViewMode::Grid,
            },
            Screen::Search => Pane::Search,
            _ => Pane::Other,
        }
    };
    keymap::Context {
        pane,
        overlay: state.export_prompt || state.reader_picker.open,
        warnings: state.show_warnings,
    }
}
fn handle_keyboard(
    state: &mut State,
    event: iced::keyboard::Event,
    services: &dyn Services,
) -> Task<Message> {
    use iced::keyboard::Event;
    match event {
        Event::ModifiersChanged(modifiers) => state.modifiers = modifiers,
        Event::KeyPressed {
            key,
            physical_key,
            modifiers,
            repeat,
            ..
        } => {
            state.modifiers = modifiers;
            if state.closing {
                return Task::none();
            }
            let context = key_context(state);
            if let Some(command) = keymap::resolve(&key, &physical_key, modifiers, &context) {
                // Holding a key repeats movement only; it must not resend or toggle.
                if repeat && !matches!(command, keymap::Command::Move { .. }) {
                    return Task::none();
                }
                return run_command(state, command, services);
            }
        }
        _ => {}
    }
    Task::none()
}
/// Show a short hint unless the same one is already on screen.
fn hint(state: &mut State, message: &str) {
    if !state.toasts.iter().any(|t| t.message == message) {
        crate::widgets::toasts::push(state, message.to_string(), false);
    }
}
fn run_command(
    state: &mut State,
    command: keymap::Command,
    services: &dyn Services,
) -> Task<Message> {
    use keymap::Command;
    let lists = matches!(state.screen, Screen::Home { .. } | Screen::Search);
    match command {
        Command::OpenSearch if state.screen == Screen::Search => {
            iced::widget::operation::focus(search_id())
        }
        Command::OpenSearch => update(state, Message::Navigate(Screen::Search), services),
        Command::ToggleSidebar => update(state, Message::ToggleSidebar, services),
        Command::ToggleDetails => update(state, Message::ToggleDetails, services),
        Command::Escape => escape(state, services),
        Command::Focus { backwards: false } => iced::widget::operation::focus_next(),
        Command::Focus { backwards: true } => iced::widget::operation::focus_previous(),
        Command::Send => {
            if let Some(reason) = crate::widgets::device_bar::send_blocker(state) {
                hint(state, reason);
                Task::none()
            } else {
                update(state, Message::Send, services)
            }
        }
        Command::ToggleConvert => {
            if !matches!(state.screen, Screen::Home { .. }) {
                return Task::none();
            }
            if state.selection.selected.is_empty() {
                hint(state, "Select a book first, then press C to convert it.");
                return Task::none();
            }
            if state.visible_settings().details_visible {
                state.convert_menu = !state.convert_menu;
                return Task::none();
            }
            let mut settings = state.visible_settings().clone();
            settings.details_visible = true;
            state.convert_menu = true;
            update(state, Message::SaveSettings(settings), services)
        }
        Command::SelectAll if lists => {
            let order = order_of(
                &state.screen,
                &state.search_order,
                &state.catalog,
                &state.facet_order,
            );
            state.selection.select_all(order);
            selection_changed(state, services)
        }
        Command::SelectAll => Task::none(),
        Command::OpenDetails if state.screen == Screen::Search => {
            if state.selection.selected.is_empty() {
                return move_cursor(state, keymap::Motion::Home, false, services);
            }
            // Details exist only in the library: show the selection there.
            state.search_return = None;
            let navigate = update(
                state,
                Message::Navigate(Screen::Home {
                    filter: Filter::All,
                }),
                services,
            );
            let reveal = reveal_cursor(state);
            Task::batch([navigate, reveal, open_details_focused(state, services)])
        }
        Command::OpenDetails if lists => {
            if state.selection.selected.is_empty() {
                return move_cursor(state, keymap::Motion::Home, false, services);
            }
            open_details_focused(state, services)
        }
        Command::OpenDetails => Task::none(),
        Command::Move { motion, extend } => move_cursor(state, motion, extend, services),
    }
}
/// Show the details pane for the selection, scrolled back to the top.
fn open_details_focused(state: &mut State, services: &dyn Services) -> Task<Message> {
    Task::batch([
        open_selected_details(state, services),
        iced::widget::operation::snap_to(
            crate::widgets::details::content_id(),
            iced::widget::scrollable::RelativeOffset::START,
        ),
    ])
}
/// Esc closes the innermost layer: prompt, popover, menu, warning list; then
/// leaves Search; then clears the selection.
fn escape(state: &mut State, services: &dyn Services) -> Task<Message> {
    if state.export_prompt {
        return update(state, Message::CancelConvert, services);
    }
    if state.reader_picker.open {
        state.reader_picker.close();
    } else if state.convert_menu {
        state.convert_menu = false;
    } else if state.show_warnings {
        state.show_warnings = false;
    } else if state.screen == Screen::Search {
        let back = state.search_return.take().unwrap_or(Screen::Home {
            filter: Filter::All,
        });
        return update(state, Message::Navigate(back), services);
    } else if matches!(state.screen, Screen::Home { .. }) {
        clear_selection(state);
    }
    Task::none()
}
fn selection_changed(state: &mut State, services: &dyn Services) -> Task<Message> {
    if matches!(state.screen, Screen::Home { .. }) {
        open_selected_details(state, services)
    } else {
        load_visible_covers(state, services)
    }
}
/// Whether the grid is shown, its column count, and the row height.
fn layout_metrics(state: &State) -> (bool, usize, f32) {
    if state.screen != Screen::Search
        && state.visible_settings().view == crate::settings::ViewMode::Grid
    {
        let geometry = crate::widgets::book_grid::Geometry::for_state(state);
        (
            true,
            geometry.columns(state.viewport_width),
            geometry.row_height,
        )
    } else {
        (false, 1, state.row_height())
    }
}
fn move_cursor(
    state: &mut State,
    motion: keymap::Motion,
    extend: bool,
    services: &dyn Services,
) -> Task<Message> {
    use keymap::Motion;
    let (grid, columns, row_height) = layout_metrics(state);
    let page = ((state.list_height / row_height).floor() as isize).max(1) * columns as isize;
    let step = if grid { columns as isize } else { 1 };
    // `None`: the key means nothing in this layout (Left/Right in a list).
    let delta = match motion {
        Motion::Up => Some(-step),
        Motion::Down => Some(step),
        Motion::Left if grid => Some(-1),
        Motion::Right if grid => Some(1),
        Motion::PageUp => Some(-page),
        Motion::PageDown => Some(page),
        Motion::Home | Motion::End => Some(0),
        Motion::Left | Motion::Right => None,
    };
    let Some(delta) = delta else {
        return Task::none();
    };
    let modifiers = if extend {
        iced::keyboard::Modifiers::SHIFT
    } else {
        iced::keyboard::Modifiers::empty()
    };
    let order = order_of(
        &state.screen,
        &state.search_order,
        &state.catalog,
        &state.facet_order,
    );
    let moved = match motion {
        Motion::Home => state.selection.move_to(0, modifiers, order),
        Motion::End => state
            .selection
            .move_to(order.len().saturating_sub(1), modifiers, order),
        _ => state.selection.move_cursor(delta, modifiers, order),
    };
    let Some(index) = moved else {
        return Task::none();
    };
    state.keyboard_focus = true;
    Task::batch([
        selection_changed(state, services),
        scroll_to_index(state, index),
    ])
}
/// Scroll just enough to bring the row or card holding `index` into view.
fn scroll_to_index(state: &mut State, index: usize) -> Task<Message> {
    let (_, columns, row_height) = layout_metrics(state);
    let top = (index / columns) as f32 * row_height;
    let height = state.list_height.max(row_height);
    let offset = if top < state.list_offset {
        top
    } else if top + row_height > state.list_offset + height {
        top + row_height - height
    } else {
        return Task::none();
    };
    state.list_offset = offset;
    iced::widget::operation::scroll_to(
        crate::widgets::book_list::id(),
        iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: offset },
    )
}
fn reveal_cursor(state: &mut State) -> Task<Message> {
    let order = order_of(
        &state.screen,
        &state.search_order,
        &state.catalog,
        &state.facet_order,
    );
    match state.selection.cursor_index(order) {
        Some(index) => scroll_to_index(state, index),
        None => Task::none(),
    }
}

fn poll(state: &mut State, services: &dyn Services) {
    for update in services.poll() {
        let id = update.view.id;
        let is_scan = update.view.kind == crate::services::jobs::JobKind::Scan;
        let cancelled = update.view.state == JobState::Cancelled;
        let is_send = update.view.kind == crate::services::jobs::JobKind::Send;
        let title = update.view.title.clone();
        let already_finished = state.finished_jobs.contains(&id);
        state.jobs.insert(id, update.view);
        if let Some(result) = update.result {
            if !already_finished {
                state.finished_jobs.push_back(id);
                if is_send {
                    let (message, error) = match result.as_ref() {
                        _ if cancelled => (format!("Cancelled · {title}"), false),
                        Ok(JobOutput::Sent(_)) => (
                            format!(
                                "Sent to {}",
                                state
                                    .send_readers
                                    .get(&id)
                                    .map(String::as_str)
                                    .unwrap_or("reader")
                            ),
                            false,
                        ),
                        Err(error) => (error.clone(), true),
                        _ => ("The send returned no destination. Try again.".into(), true),
                    };
                    state.job_messages.insert(id, message.clone());
                    crate::widgets::toasts::push(state, message, error);
                }
            }
            if state.active_scan == Some(id) {
                state.active_scan = None;
                match result.as_ref() {
                    Ok(JobOutput::Scan(report)) => {
                        state.books = report.books.clone();
                        state.catalog = crate::catalog::Catalog::new(report.books.clone());
                        refresh_filter(state);
                        invalidate_search(state);
                        state.selection.retain(&state.catalog.order);
                        if state.selection.selected.len() == 1 {
                            let book = state.selection.selected.first().unwrap().clone();
                            if let Some(entry) = state.catalog.get(&book) {
                                let key = state.metadata_guard.request(book);
                                let id = services.read_metadata(entry.book.path.clone());
                                state.metadata_requests.insert(id, key);
                            }
                        }
                        state.list_offset = 0.0;
                        state.scan_warnings =
                            report.warnings.iter().map(ToString::to_string).collect();
                        if let Some(first) = state.scan_warnings.first() {
                            log::warn!(
                                "Library scan finished with {} warnings; first: {first}",
                                state.scan_warnings.len()
                            );
                        }
                        log::info!(
                            "Library scan finished: {} books, {} warnings",
                            state.books.len(),
                            state.scan_warnings.len()
                        );
                        state.scan_status = ScanStatus::Ready;
                    }
                    Err(_) if cancelled => state.scan_status = ScanStatus::Cancelled,
                    Err(error) => {
                        state.scan_status =
                            ScanStatus::Failed(format!("Could not scan the library. {error}"))
                    }
                    _ => {
                        state.scan_status = ScanStatus::Failed(
                            "The scan returned no library data. Try again.".into(),
                        )
                    }
                }
            }
            if let Some(key) = state.metadata_requests.remove(&id)
                && state.metadata_guard.accepts(&key)
            {
                match result.as_ref() {
                    Ok(JobOutput::Metadata(metadata)) => {
                        state.metadata = Some(if let Some(entry) = state.catalog.get(&key.book) {
                            metadata
                                .clone()
                                .with_fallback(&entry.book.title, &entry.book.author)
                        } else {
                            metadata.clone()
                        });
                    }
                    Err(error) => {
                        log::warn!("Could not read book metadata: {error}");
                        state.metadata_error = Some(error.clone());
                    }
                    _ => {}
                }
            }
            if state.backend_job == Some(id) {
                state.backend_job = None;
                match result.as_ref() {
                    Ok(JobOutput::Backends(backends)) => state.backends = backends.clone(),
                    Err(error) => state.backend_error = Some(error.clone()),
                    _ => state.backend_error = Some("Converter discovery returned no data".into()),
                }
            }
            if !already_finished && state.jobs[&id].kind == crate::services::jobs::JobKind::Convert
            {
                let (message, error, folder) = match result.as_ref() {
                    Ok(JobOutput::Conversion(converted)) => {
                        let folder = converted
                            .output_path
                            .parent()
                            .unwrap_or(&converted.output_path)
                            .to_owned();
                        (
                            format!("Converted · saved to {}", folder.display()),
                            false,
                            Some(folder),
                        )
                    }
                    _ if cancelled => (format!("Cancelled · {title}"), false, None),
                    Err(error) => (error.clone(), true, None),
                    _ => ("Conversion returned no file".into(), true, None),
                };
                state
                    .job_messages
                    .insert(id, format!("{message} · Outside the library"));
                crate::widgets::toasts::push(state, message, error);
                if let Some(toast) = state.toasts.last_mut() {
                    toast.folder = folder;
                }
            }
            // Scan data is held by the active catalog only; obsolete scans must
            // not retain another full library in the generic result history.
            if !is_scan {
                state.results.insert(id, result);
            }
        }
    }
    while state.finished_jobs.len() > crate::widgets::jobs::HISTORY_LIMIT {
        let id = state.finished_jobs.pop_front().unwrap();
        state.jobs.remove(&id);
        state.results.remove(&id);
        state.job_messages.remove(&id);
        state.send_readers.remove(&id);
        state.send_devices.remove(&id);
    }
}
pub fn subscription(state: &State) -> Subscription<Message> {
    let timer = if (state
        .jobs
        .values()
        .any(|job| matches!(job.state, JobState::Waiting | JobState::Running))
        || state.active_scan.is_some()
        || state.scroll_visible
        || !state.metadata_requests.is_empty()
        || state.toasts.iter().any(|t| !t.error))
        && !state.closing
    {
        iced::time::every(Duration::from_millis(100)).map(Message::Clock)
    } else {
        Subscription::none()
    };
    Subscription::batch([
        timer,
        iced::window::close_requests().map(Message::Close),
        iced::event::listen_with(|event, status, _| match event {
            iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                Some(Message::PointerMoved(position.x))
            }
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                Some(Message::FinishDetailsResize)
            }
            iced::Event::Window(iced::window::Event::Resized(size)) => {
                Some(Message::WindowResized(size))
            }
            iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => Some(
                Message::Keyboard(iced::keyboard::Event::ModifiersChanged(m)),
            ),
            // Text fields capture typed characters and arrows; Esc still leaves Search.
            iced::Event::Keyboard(event)
                if status == iced::event::Status::Ignored
                    || matches!(
                        event,
                        iced::keyboard::Event::KeyPressed {
                            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                            ..
                        }
                    ) =>
            {
                Some(Message::Keyboard(event))
            }
            iced::Event::Mouse(iced::mouse::Event::WheelScrolled { .. }) => {
                Some(Message::ScrollActivity)
            }
            iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::ResetModifiers),
            _ => None,
        }),
    ])
}

pub fn view(state: &State) -> Element<'_, Message> {
    let content: Element<'_, Message> = if state.library_loading {
        container(text("Loading library setting…"))
            .center_x(Fill)
            .center_y(Fill)
            .into()
    } else if state.screen == Screen::Setup {
        let busy = state.setup_busy;
        let input = text_input("D:\\Calibre Library", &state.setup_path)
            .padding(10)
            .style(theme::input)
            .on_input_maybe((!busy).then_some(Message::SetupPathChanged))
            .on_submit_maybe((!busy).then_some(Message::ContinueSetup));
        let card = column![
            text("Choose your Calibre library").size(theme::font(state, 24.0)),
            text("Select the folder containing metadata.db and your books.")
                .size(theme::font(state, 14.0)),
            text("Library folder").size(theme::font(state, 14.0)),
            row![
                input,
                button("Browse…")
                    .style(theme::outline)
                    .padding(10)
                    .on_press_maybe((!busy).then_some(Message::SetupBrowse))
            ]
            .spacing(8),
            text(state.setup_error.as_deref().unwrap_or(if busy {
                "Checking and saving library…"
            } else {
                ""
            }))
            .size(theme::font(state, 13.0))
            .color(theme::DARK.warning),
            row![
                iced::widget::Space::new().width(Fill),
                button("Continue")
                    .style(theme::primary)
                    .padding(10)
                    .on_press_maybe(
                        (!busy && !state.setup_path.trim().is_empty())
                            .then_some(Message::ContinueSetup)
                    )
            ],
        ]
        .spacing(12);
        container(
            container(card)
                .padding(24)
                .max_width(500)
                .style(theme::card),
        )
        .padding(20)
        .center_x(Fill)
        .center_y(Fill)
        .into()
    } else if state.screen == Screen::Jobs {
        crate::widgets::jobs::view(state)
    } else if state.screen == Screen::Search {
        crate::widgets::book_list::view(state)
    } else if state.screen == Screen::Settings {
        crate::widgets::settings::view(state)
    } else {
        crate::widgets::book_list::view(state)
    };
    crate::widgets::shell::view(state, content)
}

#[cfg(test)]
mod tests {
    fn fixture_reader(id: &str, path: &str) -> rulibre_core::device::Device {
        use rulibre_core::device::{Device, DeviceId, DeviceKind};
        Device {
            id: DeviceId(id.into()),
            kind: DeviceKind::Kobo,
            mount_point: path.into(),
            label: None,
            filesystem: "FAT32".into(),
            read_only: false,
            evidence: vec![],
        }
    }
    #[test]
    fn reader_feed_selection_menu_escape_and_close_follow_app_messages() {
        use crate::services::devices::DeviceUpdate;
        use rulibre_core::device::DeviceEvent;
        let fake = Fake::default();
        let mut state = State::default();
        let a = fixture_reader("a", "a");
        let b = fixture_reader("b", "b");
        let snapshot = DeviceUpdate {
            revision: 0,
            event: Some(DeviceEvent::Snapshot {
                devices: vec![a.clone(), b.clone()],
                selected_id: None,
                changes: vec![],
                warnings: vec!["warning".into()],
            }),
            space: vec![],
        };
        step(&mut state, Message::Devices(snapshot.clone()), &fake);
        assert!(state.readers.selected().is_none());
        step(&mut state, Message::ToggleReaderMenu, &fake);
        assert!(state.reader_picker.open);
        step(&mut state, Message::SelectReader(b.id.clone()), &fake);
        assert_eq!(state.readers.selected(), Some(&b));
        assert!(!state.reader_picker.open);
        step(&mut state, Message::Devices(snapshot.clone()), &fake);
        assert_eq!(state.readers.selected(), Some(&b));
        step(&mut state, Message::ToggleReaderMenu, &fake);
        step(
            &mut state,
            Message::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                repeat: false,
                physical_key: iced::keyboard::key::Physical::Code(
                    iced::keyboard::key::Code::Escape,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers: iced::keyboard::Modifiers::empty(),
                text: None,
            }),
            &fake,
        );
        assert!(!state.reader_picker.open);
        step(
            &mut state,
            Message::Close(iced::window::Id::unique()),
            &fake,
        );
        step(
            &mut state,
            Message::Devices(DeviceUpdate {
                revision: 0,
                event: Some(DeviceEvent::PollFailed {
                    diagnostic: "late failure".into(),
                }),
                space: vec![],
            }),
            &fake,
        );
        assert_eq!(state.readers.selected(), Some(&b));
        assert!(*fake.stopped.lock().unwrap());
    }
    #[test]
    fn manual_reader_validation_failure_cancel_and_stale_replies() {
        use rulibre_core::device::DeviceKind;
        let fake = Fake::default();
        let mut state = State::default();
        step(&mut state, Message::ManualReader, &fake);
        step(&mut state, Message::ReaderBrowse, &fake);
        let generation = state.reader_picker.generation;
        step(
            &mut state,
            Message::ReaderFolderPicked {
                generation,
                result: Ok(None),
            },
            &fake,
        );
        assert!(!state.reader_picker.busy);
        step(&mut state, Message::ReaderKind(DeviceKind::Kindle), &fake);
        step(
            &mut state,
            Message::ReaderPath(" \"reader root\" ".into()),
            &fake,
        );
        step(&mut state, Message::ValidateReader, &fake);
        let generation = state.reader_picker.generation;
        assert_eq!(
            *fake.reader_validations.lock().unwrap(),
            [(
                std::env::current_dir().unwrap().join("reader root"),
                DeviceKind::Kindle
            )]
        );
        step(
            &mut state,
            Message::ReaderValidated {
                generation,
                result: Err("not a mount root".into()),
            },
            &fake,
        );
        assert_eq!(
            state.reader_picker.error.as_deref(),
            Some("not a mount root")
        );
        assert!(state.readers.selected().is_none());
        step(&mut state, Message::ValidateReader, &fake);
        let generation = state.reader_picker.generation;
        step(&mut state, Message::CloseReaderMenu, &fake);
        step(&mut state, Message::ManualReader, &fake);
        step(
            &mut state,
            Message::ReaderValidated {
                generation,
                result: Ok(fixture_reader("old", "old")),
            },
            &fake,
        );
        assert!(fake.remembered_readers.lock().unwrap().is_empty());
        step(&mut state, Message::ValidateReader, &fake);
        let generation = state.reader_picker.generation;
        let reader = fixture_reader("manual", "reader root");
        step(
            &mut state,
            Message::ReaderValidated {
                generation,
                result: Ok(reader.clone()),
            },
            &fake,
        );
        assert_eq!(state.readers.selected(), Some(&reader));
        assert_eq!(fake.remembered_readers.lock().unwrap().len(), 1);
        assert!(!state.reader_picker.open);
    }
    #[test]
    fn facets_navigation_and_selection_use_visible_order() {
        let fake = Fake::default();
        let books = (0..6)
            .map(|id| Book {
                id: BookId::Calibre(id),
                title: format!("{id}"),
                author: if id % 2 == 0 { "Alice" } else { "Bob" }.into(),
                files: vec![],
                path: "book".into(),
            })
            .collect();
        let mut state = State {
            screen: Screen::Home {
                filter: Filter::All,
            },
            catalog: crate::catalog::Catalog::new(books),
            scan_status: ScanStatus::Ready,
            ..Default::default()
        };
        state.sidebar_expansion.library = false;
        for group in [
            FacetGroup::Library,
            FacetGroup::Formats,
            FacetGroup::Authors,
        ] {
            step(&mut state, Message::ToggleFacetGroup(group), &fake);
        }
        assert!(
            state.sidebar_expansion.library
                && state.sidebar_expansion.formats
                && state.sidebar_expansion.authors
        );
        step(&mut state, Message::BookClicked(BookId::Calibre(1)), &fake);
        step(
            &mut state,
            Message::Navigate(Screen::Home {
                filter: Filter::Author("ALICE".into()),
            }),
            &fake,
        );
        assert_eq!(state.visible_order(), [0, 2, 4].map(BookId::Calibre));
        assert!(state.selection.selected.contains(&BookId::Calibre(1)));
        state.modifiers = iced::keyboard::Modifiers::SHIFT;
        step(&mut state, Message::BookClicked(BookId::Calibre(4)), &fake);
        assert_eq!(state.selection.selected, [BookId::Calibre(4)].into());
        state.modifiers = iced::keyboard::Modifiers::empty();
        step(&mut state, Message::BookClicked(BookId::Calibre(0)), &fake);
        state.modifiers = iced::keyboard::Modifiers::SHIFT;
        step(&mut state, Message::BookClicked(BookId::Calibre(4)), &fake);
        assert_eq!(
            state.selection.selected,
            [0, 2, 4].map(BookId::Calibre).into()
        );
        step(&mut state, Message::BookClicked(BookId::Calibre(1)), &fake);
        assert_eq!(state.selection.selected.len(), 3);
        for screen in [
            Screen::Search,
            Screen::Jobs,
            Screen::Settings,
            Screen::Home {
                filter: Filter::All,
            },
        ] {
            step(&mut state, Message::Navigate(screen.clone()), &fake);
            assert_eq!(state.screen, screen);
        }
        assert!(state.sidebar_expansion.authors);
    }
    #[test]
    fn search_coalesces_rejects_stale_a_b_a_and_restores_query() {
        let fake = Fake::default();
        let mut state = State {
            screen: Screen::Search,
            scan_status: ScanStatus::Ready,
            ..Default::default()
        };
        step(&mut state, Message::SearchChanged("A".into()), &fake);
        let first = state.search_running.unwrap();
        step(&mut state, Message::SearchChanged("B".into()), &fake);
        step(&mut state, Message::SearchChanged("A".into()), &fake);
        assert_eq!(state.search_running, Some(first));
        step(
            &mut state,
            Message::SearchFinished {
                generation: first,
                result: Ok(vec![BookId::Calibre(1)]),
            },
            &fake,
        );
        assert!(state.search_order.is_empty());
        let latest = state.search_running.unwrap();
        assert_ne!(first, latest);
        step(
            &mut state,
            Message::SearchFinished {
                generation: first,
                result: Err("old".into()),
            },
            &fake,
        );
        assert_eq!(state.search_running, Some(latest));
        step(
            &mut state,
            Message::SearchFinished {
                generation: latest,
                result: Ok(vec![BookId::Calibre(2)]),
            },
            &fake,
        );
        assert_eq!(state.search_order, [BookId::Calibre(2)]);
        state.settings.view = crate::settings::ViewMode::Grid;
        step(
            &mut state,
            Message::Navigate(Screen::Home {
                filter: Filter::All,
            }),
            &fake,
        );
        step(&mut state, Message::Navigate(Screen::Search), &fake);
        assert_eq!(state.search_query, "A");
        assert_eq!(state.visible_order(), [BookId::Calibre(2)]);
        assert!(!crate::widgets::shell::Layout::for_state(&state).details);
        step(&mut state, Message::SearchChanged("B".into()), &fake);
        let old = state.search_running.unwrap();
        step(&mut state, Message::SearchChanged("".into()), &fake);
        step(
            &mut state,
            Message::SearchFinished {
                generation: old,
                result: Ok(vec![BookId::Calibre(1)]),
            },
            &fake,
        );
        assert!(state.search_order.is_empty());
        assert!(state.search_running.is_none());
    }
    #[test]
    fn rescan_invalidates_search_snapshot_and_search_selection_stays_hidden() {
        let fake = Fake::default();
        let mut state = State {
            screen: Screen::Search,
            library: Some("library".into()),
            scan_status: ScanStatus::Ready,
            ..Default::default()
        };
        step(&mut state, Message::SearchChanged("Book".into()), &fake);
        let old = state.search_running.unwrap();
        step(&mut state, Message::Rescan, &fake);
        let scan = state.active_scan.unwrap();
        step(
            &mut state,
            Message::SearchFinished {
                generation: old,
                result: Ok(vec![BookId::Calibre(99)]),
            },
            &fake,
        );
        assert!(state.search_running.is_none());
        fake.updates.lock().unwrap().push(scan_reply(
            scan,
            Ok(rulibre_core::scanner::ScanReport {
                books: vec![Book {
                    id: BookId::Calibre(2),
                    title: "Book".into(),
                    author: "Author".into(),
                    files: vec![],
                    path: "book".into(),
                }],
                warnings: vec![],
            }),
            JobState::Succeeded,
        ));
        step(&mut state, Message::Tick, &fake);
        let new = state.search_running.unwrap();
        step(
            &mut state,
            Message::SearchFinished {
                generation: new,
                result: Ok(vec![BookId::Calibre(2)]),
            },
            &fake,
        );
        state.settings.view = crate::settings::ViewMode::List;
        state.settings.details_visible = false;
        step(&mut state, Message::BookClicked(BookId::Calibre(2)), &fake);
        assert_eq!(state.selection.selected.len(), 1);
        assert!(!state.settings.details_visible);
        step(&mut state, Message::ChangeLibrary, &fake);
        assert!(state.selection.selected.is_empty());
        step(
            &mut state,
            Message::Close(iced::window::Id::unique()),
            &fake,
        );
        assert!(start_search(&mut state).units() == 0);
    }
    #[test]
    fn home_selection_keyboard_focus_and_rescan_identity() {
        use iced::keyboard::{Event, Key, Modifiers, key::Named};
        let fake = Fake::default();
        let books: Vec<_> = (0..30)
            .map(|id| Book {
                id: BookId::Calibre(id),
                author: "Author".into(),
                title: format!("Title {id:02}"),
                files: vec![],
                path: format!("book{id}").into(),
            })
            .collect();
        let mut state = State {
            screen: Screen::Home {
                filter: Filter::All,
            },
            library: Some("library".into()),
            catalog: crate::catalog::Catalog::new(books.clone()),
            scan_status: ScanStatus::Ready,
            ..Default::default()
        };
        state.settings.view = crate::settings::ViewMode::List;
        state.settings.details_visible = false;
        step(&mut state, Message::BookClicked(BookId::Calibre(1)), &fake);
        assert!(state.visible_settings().details_visible);
        step(
            &mut state,
            Message::Keyboard(Event::ModifiersChanged(Modifiers::CTRL)),
            &fake,
        );
        step(&mut state, Message::BookClicked(BookId::Calibre(3)), &fake);
        assert_eq!(state.selection.selected.len(), 2);
        step(&mut state, Message::ResetModifiers, &fake);
        assert_eq!(state.modifiers, Modifiers::empty());
        step(
            &mut state,
            Message::Keyboard(Event::KeyPressed {
                key: Key::Named(Named::ArrowDown),
                modified_key: Key::Named(Named::ArrowDown),
                physical_key: iced::keyboard::key::Physical::Code(
                    iced::keyboard::key::Code::ArrowDown,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers: Modifiers::SHIFT,
                text: None,
                repeat: false,
            }),
            &fake,
        );
        assert_eq!(
            state.selection.selected,
            [BookId::Calibre(3), BookId::Calibre(4)].into()
        );
        assert!(state.list_offset > 0.0);
        step(&mut state, Message::Rescan, &fake);
        assert_eq!(state.selection.selected.len(), 2);
        let id = state.active_scan.unwrap();
        fake.updates.lock().unwrap().push(scan_reply(
            id,
            Ok(rulibre_core::scanner::ScanReport {
                books: books
                    .into_iter()
                    .filter(|b| b.id != BookId::Calibre(3))
                    .collect(),
                warnings: vec![],
            }),
            JobState::Succeeded,
        ));
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.selection.selected, [BookId::Calibre(4)].into());
        assert!(state.selection.anchor.is_none());
        step(&mut state, Message::ClearSelection, &fake);
        assert!(state.selection.selected.is_empty());
    }
    use super::*;
    use crate::services::{
        ServiceFuture,
        jobs::{JobKind, JobUpdate},
    };
    use std::sync::Mutex;
    #[derive(Default)]
    struct Fake {
        reader_validations: Mutex<Vec<(PathBuf, rulibre_core::device::DeviceKind)>>,
        remembered_readers: Mutex<Vec<rulibre_core::device::Device>>,
        thumbnails: Mutex<Vec<BookId>>,
        requests: Mutex<Vec<JobRequest>>,
        updates: Mutex<Vec<JobUpdate>>,
        saves: Mutex<Vec<UiSettings>>,
        cancelled: Mutex<Vec<JobId>>,
        stopped: Mutex<bool>,
        library_saves: Mutex<Vec<PathBuf>>,
    }
    fn conversion_state() -> State {
        use rulibre_core::{
            book::BookFile,
            converter::{Backend, BackendKind, BackendStatus},
        };
        let books = (1..=2)
            .map(|id| Book {
                id: BookId::Calibre(id),
                title: format!("Book {id}"),
                author: "Author".into(),
                path: format!("library/Author/Book ({id})").into(),
                files: vec![BookFile {
                    path: format!("library/Author/Book ({id})/Book.txt").into(),
                    format: Format::Txt,
                }],
            })
            .collect();
        State {
            library: Some("library".into()),
            screen: Screen::Home {
                filter: Filter::All,
            },
            catalog: crate::catalog::Catalog::new(books),
            backends: vec![Backend {
                kind: BackendKind::EbookConvert,
                path: Some("fake-converter".into()),
                version: None,
                status: BackendStatus::Ready,
                diagnostic: "fixture backend".into(),
                kepub_output: false,
            }],
            ..Default::default()
        }
    }
    #[test]
    fn window_translucency_follows_opacity_and_blur_and_never_flashes() {
        let fake = Fake::default();
        let mut state = State::default();
        state.settings.window_opacity = 100;
        state.settings.window_blur = true;
        // Not ready (non-Windows / no DirectComposition): nothing is requested.
        step(&mut state, Message::SetWindowOpacity(60), &fake);
        assert!(!state.blur.pending && !state.blur.applied);
        assert_eq!(blur_alpha(&state), 1.0);
        // Ready: a backdrop request goes out, but fills stay opaque until it is confirmed.
        state.blur.ready = true;
        let _ = sync_blur(&mut state);
        assert!(state.blur.pending);
        assert_eq!(blur_alpha(&state), 1.0);
        assert_eq!(
            style(&state, &theme::dark()).background_color,
            theme::DARK.background
        );
        step(
            &mut state,
            Message::BlurApplied {
                on: true,
                result: Ok(()),
            },
            &fake,
        );
        assert!(state.blur.applied && !state.blur.pending);
        assert!((blur_alpha(&state) - 0.6).abs() < 1e-6);
        assert_eq!(
            style(&state, &theme::dark()).background_color,
            iced::Color::TRANSPARENT
        );
        // The slider is live and clamped; no new OS call while the backdrop stays.
        step(&mut state, Message::SetWindowOpacity(0), &fake);
        assert_eq!(state.visible_settings().window_opacity, 10);
        assert!((blur_alpha(&state) - 0.1).abs() < 1e-6);
        let _ = sync_blur(&mut state);
        assert!(!state.blur.pending);
        // Blur off: translucent at once (sharp desktop), the backdrop is cleared.
        step(&mut state, Message::SetWindowBlur(false), &fake);
        assert!((blur_alpha(&state) - 0.1).abs() < 1e-6);
        let _ = sync_blur(&mut state);
        assert!(state.blur.pending);
        step(
            &mut state,
            Message::BlurApplied {
                on: false,
                result: Ok(()),
            },
            &fake,
        );
        assert!(!state.blur.applied);
        assert!((blur_alpha(&state) - 0.1).abs() < 1e-6);
        // 100 % is opaque whatever the blur setting says.
        step(&mut state, Message::SetWindowBlur(true), &fake);
        step(&mut state, Message::SetWindowOpacity(100), &fake);
        assert_eq!(blur_alpha(&state), 1.0);
        // A refusal stays opaque for the session and tells the user once.
        step(&mut state, Message::SetWindowOpacity(50), &fake);
        state.blur.pending = false;
        let _ = sync_blur(&mut state);
        assert!(state.blur.pending);
        step(
            &mut state,
            Message::BlurApplied {
                on: true,
                result: Err("old Windows".into()),
            },
            &fake,
        );
        assert!(state.blur.failed && !state.blur.applied);
        assert_eq!(blur_alpha(&state), 1.0);
        assert_eq!(state.toasts.len(), 1);
        let _ = sync_blur(&mut state);
        assert!(!state.blur.pending);
        // The settings still persist even when the effect is unavailable.
        assert_eq!(state.visible_settings().window_opacity, 50);
        assert!(state.visible_settings().window_blur);
    }
    #[test]
    fn scrollbars_show_on_activity_and_hide_after_five_idle_seconds() {
        let fake = Fake::default();
        let mut state = State::default();
        assert!(!state.scroll_visible);
        step(&mut state, Message::ScrollActivity, &fake);
        assert!(state.scroll_visible);
        let last = state.scroll_last.unwrap();
        step(
            &mut state,
            Message::Clock(last + Duration::from_millis(4999)),
            &fake,
        );
        assert!(state.scroll_visible);
        step(
            &mut state,
            Message::Clock(last + Duration::from_secs(5)),
            &fake,
        );
        assert!(!state.scroll_visible);
        // Scrolling by keyboard or by dragging the bar reports a list offset.
        step(
            &mut state,
            Message::ListScrolled {
                offset: 10.0,
                height: 300.0,
            },
            &fake,
        );
        assert!(state.scroll_visible);
    }
    #[test]
    fn selection_metadata_error_fallback_and_a_b_a_are_guarded() {
        let fake = Fake::default();
        let mut state = conversion_state();
        for id in [1, 2, 1] {
            step(&mut state, Message::BookClicked(BookId::Calibre(id)), &fake);
        }
        fake.updates
            .lock()
            .unwrap()
            .extend([reply(1, "Old A"), reply(2, "Old B")]);
        step(&mut state, Message::Tick, &fake);
        assert!(state.metadata.is_none());
        let mut error = reply(3, "Unused");
        error.result = Some(Arc::new(Err("Malformed OPF".into())));
        fake.updates.lock().unwrap().push(error);
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.metadata_error.as_deref(), Some("Malformed OPF"));
        assert!(state.notices.is_empty());
        let _ = crate::widgets::details::view(&state);
        step(&mut state, Message::BookClicked(BookId::Calibre(2)), &fake);
        let mut response = reply(4, "");
        response.result = Some(Arc::new(Ok(JobOutput::Metadata(Metadata::default()))));
        fake.updates.lock().unwrap().push(response);
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.metadata.as_ref().unwrap().title, "Book 2");
        assert_eq!(state.metadata.as_ref().unwrap().authors, ["Author"]);
        assert!(state.metadata_error.is_none());
    }
    #[test]
    fn convert_targets_intersect_and_diagnostics_explain_unavailable_books() {
        use crate::widgets::details::{targets, unavailable};
        use rulibre_core::converter::BackendStatus;
        let state = conversion_state();
        let mut books = crate::widgets::details::selected(&state);
        assert!(books.is_empty());
        books = state
            .catalog
            .order
            .iter()
            .map(|id| state.catalog.get(id).unwrap().book.clone())
            .collect();
        assert!(targets(&books, &state.backends).contains(&Format::Epub));
        assert!(!targets(&books, &state.backends).contains(&Format::Txt));
        assert!(unavailable(&books[0], Format::Kepub, &state.backends).contains("fixture backend"));
        books[1].files[0].format = Format::Kepub;
        assert!(targets(&books, &state.backends).is_empty());
        let reason = unavailable(&books[1], Format::Epub, &state.backends);
        assert!(reason.contains("no supported route"));
        for status in [BackendStatus::Missing, BackendStatus::Broken] {
            let mut backends = state.backends.clone();
            backends[0].status = status;
            assert!(targets(&books[..1], &backends).is_empty());
            assert!(
                unavailable(&books[0], Format::Epub, &backends).contains(&format!("{status:?}"))
            );
        }
    }
    #[test]
    fn first_convert_captures_books_waits_for_folder_save_and_enqueues_each_book() {
        let fake = Fake::default();
        let mut state = conversion_state();
        state.selection.selected.extend(state.catalog.order.clone());
        step(&mut state, Message::Convert(Format::Epub), &fake);
        assert!(state.export_prompt);
        assert!(fake.requests.lock().unwrap().is_empty());
        step(&mut state, Message::ClearSelection, &fake);
        state.conversion_export = Some("export".into());
        step(&mut state, Message::UseExport, &fake);
        assert!(fake.requests.lock().unwrap().is_empty());
        let settings = state.visible_settings().clone();
        step(
            &mut state,
            Message::SettingsSaved {
                settings,
                result: Ok(()),
            },
            &fake,
        );
        let requests = fake.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            let JobRequest::ConvertFiles {
                library,
                output_dir,
                files,
                target,
            } = request
            else {
                panic!("convert job")
            };
            assert_eq!(library, &PathBuf::from("library"));
            assert_eq!(output_dir, &PathBuf::from("export"));
            assert_eq!(*target, Format::Epub);
            assert_eq!(files.len(), 1);
        }
        assert!(state.pending_conversion.is_none());
    }
    #[test]
    fn cancelled_invalid_or_unsaved_export_never_starts_conversion() {
        let fake = Fake::default();
        let mut state = conversion_state();
        state.selection.selected.insert(BookId::Calibre(1));
        step(&mut state, Message::Convert(Format::Kepub), &fake);
        assert!(!state.export_prompt);
        step(&mut state, Message::Convert(Format::Epub), &fake);
        step(&mut state, Message::ExportBrowse, &fake);
        let generation = state.export_generation;
        step(
            &mut state,
            Message::ExportPicked {
                generation,
                result: Ok(None),
            },
            &fake,
        );
        assert!(state.export_prompt);
        step(&mut state, Message::ExportBrowse, &fake);
        let generation = state.export_generation;
        step(
            &mut state,
            Message::ExportValidated {
                generation,
                result: Err("Invalid export".into()),
            },
            &fake,
        );
        assert!(state.conversion_export.is_none());
        state.conversion_export = Some("export".into());
        step(&mut state, Message::UseExport, &fake);
        let settings = state.visible_settings().clone();
        step(
            &mut state,
            Message::SettingsSaved {
                settings,
                result: Err("Disk full".into()),
            },
            &fake,
        );
        assert!(state.pending_conversion.is_none());
        assert!(fake.requests.lock().unwrap().is_empty());
        step(&mut state, Message::Convert(Format::Epub), &fake);
        step(&mut state, Message::CancelConvert, &fake);
        assert!(!state.export_prompt);
        assert!(state.pending_conversion.is_none());
    }
    #[test]
    fn fake_conversion_result_provides_export_action_and_conflicts_are_sticky() {
        let fake = Fake::default();
        let fixture = tempfile::tempdir().unwrap();
        let mut state = conversion_state();
        state.settings.export_folder = Some(fixture.path().into());
        state.selection.selected.insert(BookId::Calibre(1));
        step(&mut state, Message::Convert(Format::Epub), &fake);
        assert_eq!(fake.requests.lock().unwrap().len(), 1);
        let output = fixture.path().join("Book.epub");
        // Synthetic backend result, not an ebook container or a real process.
        std::fs::write(&output, b"converted fixture").unwrap();
        let mut response = reply(1, "Book 1");
        response.view.kind = JobKind::Convert;
        response.result = Some(Arc::new(Ok(JobOutput::Conversion(
            rulibre_core::converter::ConversionResult {
                output_path: output.clone(),
                size: 17,
                source: "Book.txt".into(),
                target: Format::Epub,
                backend: state.backends[0].clone(),
            },
        ))));
        fake.updates.lock().unwrap().push(response);
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.toasts.len(), 1);
        assert!(state.toasts[0].message.starts_with("Converted · saved to"));
        assert_eq!(state.toasts[0].folder.as_deref(), Some(fixture.path()));
        assert!(state.job_messages[&JobId(1)].contains("Outside the library"));
        assert_eq!(state.catalog.order.len(), 2);
        let mut conflict = reply(2, "Book 1");
        conflict.view.kind = JobKind::Convert;
        conflict.view.state = JobState::Failed;
        conflict.result = Some(Arc::new(Err(crate::services::jobs::error_message(
            &rulibre_core::Error {
                operation: "convert",
                path: Some(output.clone()),
                kind: rulibre_core::ErrorKind::Conflict,
            },
        ))));
        fake.updates.lock().unwrap().push(conflict);
        step(&mut state, Message::Tick, &fake);
        assert!(state.toasts.last().unwrap().error);
        assert!(
            state
                .toasts
                .last()
                .unwrap()
                .message
                .contains("not overwritten")
        );
        assert_eq!(std::fs::read(output).unwrap(), b"converted fixture");
    }
    #[test]
    fn cover_requests_drop_scrolled_and_hidden_replies_and_persist_view() {
        let fake = Fake::default();
        let mut state = State {
            screen: Screen::Home {
                filter: Filter::All,
            },
            scan_status: ScanStatus::Ready,
            catalog: crate::catalog::Catalog::new(
                (0..5000)
                    .map(|id| Book {
                        id: BookId::Calibre(id),
                        title: format!("{id:04}"),
                        author: "Author".into(),
                        files: vec![],
                        path: format!("book{id}").into(),
                    })
                    .collect(),
            ),
            ..Default::default()
        };
        step(
            &mut state,
            Message::Viewport(iced::Size::new(732.0, 364.0)),
            &fake,
        );
        assert_eq!(fake.thumbnails.lock().unwrap().len(), 4);
        let old: Vec<_> = state
            .covers
            .running
            .iter()
            .map(|(id, generation)| (id.clone(), *generation))
            .collect();
        step(
            &mut state,
            Message::ListScrolled {
                offset: 18200.0,
                height: 364.0,
            },
            &fake,
        );
        assert_eq!(fake.thumbnails.lock().unwrap().len(), 4);
        for (id, generation) in old {
            step(
                &mut state,
                Message::ThumbnailLoaded {
                    id: id.clone(),
                    generation,
                    result: Ok(Some(iced::widget::image::Handle::from_rgba(
                        1,
                        1,
                        vec![0; 4],
                    ))),
                },
                &fake,
            );
            assert!(!state.covers.contains(&id));
        }
        assert!(state.covers.running.len() <= 4);
        let (id, generation) = state
            .covers
            .running
            .first_key_value()
            .map(|(id, generation)| (id.clone(), *generation))
            .unwrap();
        step(&mut state, Message::Navigate(Screen::Jobs), &fake);
        step(
            &mut state,
            Message::ThumbnailLoaded {
                id: id.clone(),
                generation,
                result: Ok(None),
            },
            &fake,
        );
        assert!(!state.covers.contains(&id));
        assert!(state.covers.wanted.is_empty());
        step(&mut state, Message::ShowList, &fake);
        assert_eq!(
            fake.saves.lock().unwrap().last().unwrap().view,
            crate::settings::ViewMode::List
        );
        assert_eq!(
            state.visible_settings().view,
            crate::settings::ViewMode::List
        );
    }
    impl Services for Fake {
        fn validate_reader(
            &self,
            path: PathBuf,
            kind: rulibre_core::device::DeviceKind,
        ) -> ServiceFuture<rulibre_core::device::Device> {
            self.reader_validations.lock().unwrap().push((path, kind));
            Box::pin(async { Err("fixture validation response".into()) })
        }
        fn remember_reader(&self, reader: &rulibre_core::device::Device) -> u64 {
            let mut readers = self.remembered_readers.lock().unwrap();
            readers.push(reader.clone());
            readers.len() as u64
        }
        fn load_thumbnail(
            &self,
            id: BookId,
            _: PathBuf,
        ) -> ServiceFuture<Option<iced::widget::image::Handle>> {
            self.thumbnails.lock().unwrap().push(id);
            Box::pin(async { Ok(None) })
        }
        fn load_library(&self) -> ServiceFuture<Option<PathBuf>> {
            Box::pin(async { Ok(None) })
        }
        fn validate_library(&self, input: String) -> ServiceFuture<PathBuf> {
            Box::pin(async move { Ok(input.into()) })
        }
        fn save_library(&self, path: PathBuf) -> ServiceFuture<PathBuf> {
            self.library_saves.lock().unwrap().push(path.clone());
            Box::pin(async move { Ok(path) })
        }
        fn enqueue(&self, _: String, request: JobRequest) -> JobId {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            JobId(requests.len() as u64)
        }
        fn poll(&self) -> Vec<JobUpdate> {
            std::mem::take(&mut *self.updates.lock().unwrap())
        }
        fn cancel(&self, id: JobId) {
            self.cancelled.lock().unwrap().push(id);
        }
        fn clear_finished(&self) {}
        fn shutdown(&self) {
            *self.stopped.lock().unwrap() = true;
        }
        fn load_settings(&self) -> ServiceFuture<UiSettings> {
            Box::pin(async { Err("Corrupt ui.toml".into()) })
        }
        fn save_settings(&self, settings: UiSettings) -> ServiceFuture<()> {
            self.saves.lock().unwrap().push(settings);
            Box::pin(async { Ok(()) })
        }
        fn find_cover(&self, _: PathBuf) -> ServiceFuture<Option<CoverFile>> {
            Box::pin(async { Ok(None) })
        }
        fn pick_folder(&self) -> ServiceFuture<Option<PathBuf>> {
            Box::pin(async { Ok(Some("export".into())) })
        }
    }
    fn step(state: &mut State, message: Message, services: &Fake) {
        let _ = update(state, message, services);
    }
    #[test]
    fn settings_toggles_and_export_validation_merge_and_revert_on_save_failure() {
        let fake = Fake::default();
        let mut state = State {
            screen: Screen::Settings,
            library: Some("library".into()),
            ..Default::default()
        };
        step(&mut state, Message::SetWindowOpacity(30), &fake);
        step(&mut state, Message::SetPreferKepub(false), &fake);
        assert_eq!(state.visible_settings().window_opacity, 30);
        assert!(!state.visible_settings().prefer_kepub);
        step(&mut state, Message::ExportBrowse, &fake);
        let generation = state.export_generation;
        step(
            &mut state,
            Message::ExportValidated {
                generation,
                result: Err("Inside library".into()),
            },
            &fake,
        );
        assert_eq!(state.visible_settings().export_folder, None);
        assert!(state.toasts.last().unwrap().error);
        step(&mut state, Message::ExportBrowse, &fake);
        let generation = state.export_generation;
        step(
            &mut state,
            Message::ExportValidated {
                generation,
                result: Ok("exports".into()),
            },
            &fake,
        );
        let first = fake.saves.lock().unwrap()[0].clone();
        step(
            &mut state,
            Message::SettingsSaved {
                settings: first,
                result: Ok(()),
            },
            &fake,
        );
        let merged = fake.saves.lock().unwrap()[1].clone();
        assert_eq!(merged.window_opacity, 30);
        assert!(!merged.prefer_kepub);
        assert_eq!(merged.export_folder, Some("exports".into()));
        step(
            &mut state,
            Message::SettingsSaved {
                settings: merged,
                result: Err("Disk full".into()),
            },
            &fake,
        );
        assert_eq!(state.visible_settings().export_folder, None);
        assert!(state.settings.prefer_kepub);
        assert_eq!(state.settings.window_opacity, 30);
    }
    #[test]
    fn settings_library_failure_cancel_and_switch_during_scan_keep_old_state_until_saved() {
        let fake = Fake::default();
        let mut state = State::default();
        library_reply(&mut state, &fake, Ok(Some("old".into())));
        let old_scan = state.active_scan.unwrap();
        state.screen = Screen::Settings;
        for reply in [Ok(None), Err("Picker failed".into())] {
            step(&mut state, Message::SettingsLibraryBrowse, &fake);
            let generation = state.library_generation;
            step(
                &mut state,
                Message::SettingsLibraryPicked {
                    generation,
                    result: reply,
                },
                &fake,
            );
            assert!(!state.setup_busy);
            assert_eq!(state.active_scan, Some(old_scan));
        }
        step(&mut state, Message::SettingsLibraryBrowse, &fake);
        let generation = state.library_generation;
        step(
            &mut state,
            Message::SettingsLibraryPicked {
                generation,
                result: Ok(Some("invalid".into())),
            },
            &fake,
        );
        step(
            &mut state,
            Message::LibrarySaved {
                generation,
                result: Err("Invalid Calibre library".into()),
            },
            &fake,
        );
        assert_eq!(state.library, Some("old".into()));
        assert_eq!(state.active_scan, Some(old_scan));
        assert_eq!(state.screen, Screen::Settings);
        assert!(fake.cancelled.lock().unwrap().is_empty());
        step(&mut state, Message::SettingsLibraryBrowse, &fake);
        let generation = state.library_generation;
        step(
            &mut state,
            Message::SettingsLibraryPicked {
                generation,
                result: Ok(Some("new".into())),
            },
            &fake,
        );
        step(
            &mut state,
            Message::LibrarySaved {
                generation,
                result: Ok("new".into()),
            },
            &fake,
        );
        assert_eq!(state.library, Some("new".into()));
        assert_eq!(*fake.cancelled.lock().unwrap(), [old_scan]);
        let new_scan = state.active_scan;
        fake.updates.lock().unwrap().push(scan_reply(
            old_scan,
            Err("Stale failure".into()),
            JobState::Failed,
        ));
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.active_scan, new_scan);
        assert_eq!(state.scan_status, ScanStatus::Running);
    }
    #[test]
    fn export_cancel_and_old_library_reply_do_not_save() {
        let fake = Fake::default();
        let mut state = State {
            library: Some("old".into()),
            ..Default::default()
        };
        step(&mut state, Message::ExportBrowse, &fake);
        let generation = state.export_generation;
        step(
            &mut state,
            Message::ExportPicked {
                generation,
                result: Ok(None),
            },
            &fake,
        );
        assert!(!state.export_busy);
        step(&mut state, Message::ExportBrowse, &fake);
        let generation = state.export_generation;
        activate_library(&mut state, "new".into(), &fake);
        step(
            &mut state,
            Message::ExportValidated {
                generation,
                result: Ok("old-export".into()),
            },
            &fake,
        );
        assert!(fake.saves.lock().unwrap().is_empty());
        assert_eq!(state.settings.export_folder, None);
    }
    #[test]
    fn send_uses_selection_chronology_catalog_and_captured_reader_preferences() {
        let fake = Fake::default();
        let mut state = State {
            screen: Screen::Home {
                filter: Filter::All,
            },
            catalog: crate::catalog::Catalog::new(
                (1..=3)
                    .map(|n| Book {
                        id: BookId::Calibre(n),
                        title: format!("Book {n}"),
                        author: format!("Author {n}"),
                        path: format!("book{n}").into(),
                        files: vec![],
                    })
                    .collect(),
            ),
            ..Default::default()
        };
        let reader = fixture_reader("reader", "reader-root");
        state.readers.devices = vec![reader.clone()];
        state.readers.selected_id = Some(reader.id.clone());
        for n in [3, 1, 2] {
            state.selection.click(
                BookId::Calibre(n),
                iced::keyboard::Modifiers::CTRL,
                &state.catalog.order,
            );
        }
        state.settings.prefer_kepub = true;
        step(&mut state, Message::Send, &fake);
        let requests = fake.requests.lock().unwrap();
        for (request, n) in requests.iter().zip([3, 1, 2]) {
            let JobRequest::Send {
                book,
                author,
                device,
                options,
            } = request
            else {
                panic!("send")
            };
            assert_eq!(book, &PathBuf::from(format!("book{n}")));
            assert_eq!(author, &format!("Author {n}"));
            assert_eq!(device, &reader);
            assert_eq!(
                options.kobo,
                rulibre_core::device::KoboPreparation::PreferKepub
            );
        }
        assert_eq!(requests.len(), 3);
        drop(requests);
        assert_eq!(
            state
                .jobs
                .values()
                .filter(|j| crate::widgets::jobs::unfinished(j))
                .count(),
            3
        );
        state.readers.devices[0].read_only = true;
        step(&mut state, Message::Send, &fake);
        state.readers.devices.clear();
        step(&mut state, Message::Send, &fake);
        assert_eq!(fake.requests.lock().unwrap().len(), 3);
        state.readers.devices.push(reader);
        state.selection.clear();
        step(&mut state, Message::Send, &fake);
        assert_eq!(fake.requests.lock().unwrap().len(), 3);
    }
    #[test]
    fn send_results_badge_cancel_toast_lifetime_and_history() {
        use std::time::Instant;
        let fake = Fake::default();
        let mut state = State::default();
        for n in 1..=23 {
            let terminal = JobUpdate {
                view: JobView {
                    id: JobId(n),
                    title: format!("Book {n}"),
                    kind: JobKind::Send,
                    state: if n == 2 {
                        JobState::Cancelled
                    } else if n == 3 {
                        JobState::Failed
                    } else {
                        JobState::Succeeded
                    },
                    stage: Some(rulibre_core::jobs::Stage::Copying),
                    progress: Some((42, 100)),
                    error: (n == 3).then(|| "Destination already exists: book.epub".into()),
                },
                result: Some(Arc::new(if n == 2 {
                    Err("Cancelled".into())
                } else if n == 3 {
                    Err("Destination already exists: book.epub".into())
                } else {
                    Ok(JobOutput::Sent("reader/book.epub".into()))
                })),
            };
            state.send_readers.insert(JobId(n), "Kobo".into());
            fake.updates.lock().unwrap().push(terminal);
        }
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.jobs.len(), 20);
        assert_eq!(state.results.len(), 20);
        assert_eq!(
            state
                .jobs
                .values()
                .filter(|j| crate::widgets::jobs::unfinished(j))
                .count(),
            0
        );
        assert!(
            state
                .toasts
                .iter()
                .any(|t| t.message == "Cancelled · Book 2" && !t.error)
        );
        assert!(state.toasts.iter().any(|t| t.message == "Sent to Kobo"));
        assert_eq!(state.toasts.iter().filter(|t| t.error).count(), 1);
        let now = Instant::now();
        step(&mut state, Message::Clock(now), &fake);
        step(
            &mut state,
            Message::Clock(now + Duration::from_millis(4999)),
            &fake,
        );
        assert_eq!(state.toasts.len(), 23);
        step(
            &mut state,
            Message::Clock(now + Duration::from_secs(5)),
            &fake,
        );
        assert_eq!(state.toasts.len(), 1);
        let id = state.toasts[0].id;
        step(&mut state, Message::DismissToast(id), &fake);
        assert!(state.toasts.is_empty());
        step(&mut state, Message::ClearFinished, &fake);
        assert!(state.finished_jobs.is_empty());
        assert!(state.jobs.is_empty());
        assert!(state.results.is_empty());
        step(&mut state, Message::Cancel(JobId(24)), &fake);
        assert_eq!(*fake.cancelled.lock().unwrap(), [JobId(24)]);
    }
    #[test]
    fn shell_toggles_coalesce_and_failed_save_reverts_visible_preferences() {
        let fake = Fake::default();
        let mut state = State {
            screen: Screen::Home {
                filter: Filter::All,
            },
            ..Default::default()
        };
        step(&mut state, Message::ToggleSidebar, &fake);
        let first = fake.saves.lock().unwrap()[0].clone();
        assert!(!state.visible_settings().sidebar_visible);
        step(&mut state, Message::ToggleDetails, &fake);
        step(&mut state, Message::ToggleSidebar, &fake);
        assert!(state.visible_settings().sidebar_visible);
        assert!(!state.visible_settings().details_visible);
        step(
            &mut state,
            Message::SettingsSaved {
                settings: first,
                result: Ok(()),
            },
            &fake,
        );
        let second = fake.saves.lock().unwrap()[1].clone();
        assert!(second.sidebar_visible);
        assert!(!second.details_visible);
        step(
            &mut state,
            Message::SettingsSaved {
                settings: second,
                result: Err("Cannot save".into()),
            },
            &fake,
        );
        assert_eq!(state.visible_settings(), &state.settings);
        assert!(!state.visible_settings().sidebar_visible);
        assert!(state.visible_settings().details_visible);
        assert!(state.toasts.last().unwrap().message.contains("Cannot save"));
    }
    #[test]
    fn shell_screen_rules_and_minimum_width() {
        use crate::widgets::shell::Layout;
        let fake = Fake::default();
        let mut state = State {
            screen: Screen::Home {
                filter: Filter::All,
            },
            ..Default::default()
        };
        assert_eq!(Layout::for_state(&state).content_width(760.0), 305.0);
        step(&mut state, Message::ToggleSidebar, &fake);
        assert_eq!(Layout::for_state(&state).content_width(760.0), 453.0);
        for screen in [
            Screen::Search,
            Screen::Jobs,
            Screen::Settings,
            Screen::Setup,
        ] {
            step(&mut state, Message::Navigate(screen.clone()), &fake);
            let before = state.visible_settings().clone();
            step(&mut state, Message::ToggleDetails, &fake);
            assert_eq!(&before, state.visible_settings());
            assert!(!Layout::for_state(&state).details_available);
            assert!(!Layout::for_state(&state).details);
            if screen == Screen::Setup {
                step(&mut state, Message::ToggleSidebar, &fake);
                assert_eq!(&before, state.visible_settings());
                assert_eq!(Layout::for_state(&state).content_width(760.0), 760.0);
            }
        }
        step(
            &mut state,
            Message::Navigate(Screen::Home {
                filter: Filter::All,
            }),
            &fake,
        );
        assert!(Layout::for_state(&state).details);
    }
    #[test]
    fn appearance_and_drag_save_coalesce_and_failure_reverts_width() {
        let fake = Fake::default();
        let mut state = State {
            screen: Screen::Home {
                filter: Filter::All,
            },
            window_width: 760.0,
            ..Default::default()
        };
        step(&mut state, Message::PointerMoved(460.0), &fake);
        step(&mut state, Message::StartDetailsResize, &fake);
        step(&mut state, Message::PointerMoved(350.0), &fake);
        assert_eq!(state.details_width(), 409.0);
        assert!(fake.saves.lock().unwrap().is_empty());
        step(&mut state, Message::PointerMoved(100.0), &fake);
        assert_eq!(state.details_width(), 409.0);
        assert!(crate::widgets::shell::Layout::for_state(&state).content_width(760.0) >= 180.0);
        step(&mut state, Message::FinishDetailsResize, &fake);
        assert!(state.details_drag.is_none());
        assert_eq!(fake.saves.lock().unwrap().len(), 1);
        step(&mut state, Message::SetTextSize(20), &fake);
        step(&mut state, Message::SetCoverSize(200), &fake);
        let first = fake.saves.lock().unwrap()[0].clone();
        step(
            &mut state,
            Message::SettingsSaved {
                settings: first,
                result: Ok(()),
            },
            &fake,
        );
        let last = fake.saves.lock().unwrap()[1].clone();
        assert_eq!(
            (last.text_size, last.cover_size, last.details_width),
            (20, 200, 409)
        );
        step(
            &mut state,
            Message::SettingsSaved {
                settings: last,
                result: Ok(()),
            },
            &fake,
        );
        step(&mut state, Message::PointerMoved(350.0), &fake);
        step(&mut state, Message::StartDetailsResize, &fake);
        step(&mut state, Message::PointerMoved(600.0), &fake);
        assert_eq!(state.details_width(), 240.0);
        step(&mut state, Message::FinishDetailsResize, &fake);
        let failed = state.visible_settings().clone();
        step(
            &mut state,
            Message::SettingsSaved {
                settings: failed,
                result: Err("Disk full".into()),
            },
            &fake,
        );
        assert_eq!(state.details_width(), 369.0);
        assert!(state.toasts.last().unwrap().error);
        step(
            &mut state,
            Message::WindowResized(iced::Size::new(1100.0, 700.0)),
            &fake,
        );
        assert_eq!(state.details_width(), 409.0);
        step(&mut state, Message::PointerMoved(400.0), &fake);
        step(&mut state, Message::StartDetailsResize, &fake);
        step(&mut state, Message::PointerMoved(500.0), &fake);
        step(&mut state, Message::ResetModifiers, &fake);
        assert!(state.details_drag.is_none());
        assert_eq!(state.details_width(), 409.0);
    }
    fn reply(id: u64, title: &str) -> JobUpdate {
        JobUpdate {
            view: JobView {
                id: JobId(id),
                title: title.into(),
                kind: JobKind::Metadata,
                state: JobState::Succeeded,
                progress: None,
                stage: None,
                error: None,
            },
            result: Some(Arc::new(Ok(JobOutput::Metadata(Metadata {
                title: title.into(),
                ..Default::default()
            })))),
        }
    }
    #[test]
    fn defaults_navigation_and_corrupt_settings_notice() {
        let mut state = State::default();
        let fake = Fake::default();
        assert_eq!(state.screen, Screen::Setup);
        step(
            &mut state,
            Message::SettingsLoaded(Err("Corrupt ui.toml".into())),
            &fake,
        );
        assert_eq!(state.settings, UiSettings::default());
        assert_eq!(state.notices, ["Corrupt ui.toml"]);
        assert!(fake.saves.lock().unwrap().is_empty());
        step(
            &mut state,
            Message::Navigate(Screen::Home {
                filter: Filter::Format(Format::Epub),
            }),
            &fake,
        );
        assert_eq!(
            state.screen,
            Screen::Home {
                filter: Filter::Format(Format::Epub)
            }
        );
    }
    #[test]
    fn saves_are_serialized_and_failed_save_keeps_previous_settings() {
        let mut state = State::default();
        let fake = Fake::default();
        let first = UiSettings {
            sidebar_visible: false,
            ..Default::default()
        };
        let second = UiSettings {
            window_opacity: 30,
            window_blur: false,
            ..first.clone()
        };
        step(&mut state, Message::SaveSettings(first.clone()), &fake);
        step(&mut state, Message::SaveSettings(second.clone()), &fake);
        assert_eq!(
            fake.saves.lock().unwrap().as_slice(),
            std::slice::from_ref(&first)
        );
        step(
            &mut state,
            Message::SettingsSaved {
                settings: first.clone(),
                result: Ok(()),
            },
            &fake,
        );
        assert_eq!(state.settings, first);
        assert_eq!(*fake.saves.lock().unwrap(), [first.clone(), second.clone()]);
        step(
            &mut state,
            Message::SettingsSaved {
                settings: second,
                result: Err("Disk full".into()),
            },
            &fake,
        );
        assert_eq!(state.settings, first);
        assert!(state.toasts.last().unwrap().message.contains("Disk full"));
        assert!(state.toasts.last().unwrap().error);
    }
    #[test]
    fn settings_edits_during_startup_wait_for_load() {
        let mut state = State::default();
        let fake = Fake::default();
        step(&mut state, Message::LoadSettings, &fake);
        let settings = UiSettings {
            sidebar_visible: false,
            ..Default::default()
        };
        step(&mut state, Message::SaveSettings(settings.clone()), &fake);
        assert!(fake.saves.lock().unwrap().is_empty());
        step(
            &mut state,
            Message::SettingsLoaded(Ok(UiSettings::default())),
            &fake,
        );
        assert_eq!(*fake.saves.lock().unwrap(), [settings]);
    }
    #[test]
    fn metadata_and_cover_guards_reject_a_b_a_and_hidden_results() {
        let mut state = State::default();
        let fake = Fake::default();
        for id in [1, 2, 1] {
            step(
                &mut state,
                Message::LoadMetadata {
                    book: BookId::Calibre(id),
                    path: format!("book{id}").into(),
                },
                &fake,
            );
        }
        fake.updates.lock().unwrap().push(reply(1, "Old A"));
        fake.updates.lock().unwrap().push(reply(2, "B"));
        step(&mut state, Message::Tick, &fake);
        assert!(state.metadata.is_none());
        fake.updates.lock().unwrap().push(reply(3, "Latest A"));
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.metadata.as_ref().unwrap().title, "Latest A");
        let old = state.cover_guard.request(BookId::Calibre(1));
        state.cover_guard.request(BookId::Calibre(2));
        let latest = state.cover_guard.request(BookId::Calibre(1));
        step(
            &mut state,
            Message::CoverLoaded {
                key: old,
                result: Err("stale error".into()),
            },
            &fake,
        );
        assert!(state.notices.is_empty());
        step(&mut state, Message::Navigate(Screen::Search), &fake);
        step(
            &mut state,
            Message::CoverLoaded {
                key: latest,
                result: Err("hidden error".into()),
            },
            &fake,
        );
        assert!(state.notices.is_empty());
        assert!(state.metadata.is_none());
    }
    #[test]
    fn submit_cancel_poll_and_clear_finished() {
        let mut state = State::default();
        let fake = Fake::default();
        step(
            &mut state,
            Message::Submit {
                title: "Scan".into(),
                request: JobRequest::Scan {
                    library: "library".into(),
                },
            },
            &fake,
        );
        assert!(matches!(
            fake.requests.lock().unwrap()[0],
            JobRequest::Scan { .. }
        ));
        step(&mut state, Message::Cancel(JobId(1)), &fake);
        assert_eq!(*fake.cancelled.lock().unwrap(), [JobId(1)]);
        fake.updates.lock().unwrap().push(reply(1, "A"));
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.results.len(), 1);
        step(&mut state, Message::ClearFinished, &fake);
        assert!(state.jobs.is_empty());
        assert!(state.results.is_empty());
    }
    #[test]
    fn folder_picker_result_and_cancel() {
        let mut state = State::default();
        let fake = Fake::default();
        step(&mut state, Message::PickFolder, &fake);
        step(
            &mut state,
            Message::FolderPicked(Ok(Some("export".into()))),
            &fake,
        );
        assert_eq!(state.picked_folder, Some("export".into()));
        step(&mut state, Message::FolderPicked(Ok(None)), &fake);
        assert!(state.picked_folder.is_none());
    }

    fn scan_reply(
        id: JobId,
        result: Result<rulibre_core::scanner::ScanReport, String>,
        state: JobState,
    ) -> JobUpdate {
        JobUpdate {
            view: JobView {
                id,
                title: "Scan library".into(),
                kind: JobKind::Scan,
                state,
                progress: None,
                stage: None,
                error: result.as_ref().err().cloned(),
            },
            result: Some(Arc::new(result.map(JobOutput::Scan))),
        }
    }
    fn library_reply(state: &mut State, fake: &Fake, result: Result<Option<PathBuf>, String>) {
        step(state, Message::LoadLibrary, fake);
        step(
            state,
            Message::LibraryLoaded {
                generation: state.library_generation,
                result,
            },
            fake,
        );
    }
    #[test]
    fn first_run_saved_library_and_config_errors() {
        for error in [
            None,
            Some("Saved library deleted"),
            Some("Could not load config.toml: invalid TOML"),
        ] {
            let mut state = State::default();
            let fake = Fake::default();
            library_reply(
                &mut state,
                &fake,
                error.map_or(Ok(None), |error| Err(error.into())),
            );
            assert_eq!(state.screen, Screen::Setup);
            assert_eq!(state.setup_error.as_deref(), error);
            assert!(fake.requests.lock().unwrap().is_empty());
        }
        let mut state = State::default();
        let fake = Fake::default();
        library_reply(&mut state, &fake, Ok(Some("library".into())));
        assert!(matches!(state.screen, Screen::Home { .. }));
        assert_eq!(state.scan_status, ScanStatus::Running);
        assert!(
            matches!(&fake.requests.lock().unwrap()[0], JobRequest::Scan { library } if library == &PathBuf::from("library"))
        );
    }
    #[test]
    fn setup_validation_and_save_must_succeed_before_scanning() {
        let mut state = State::default();
        let fake = Fake::default();
        step(
            &mut state,
            Message::SetupPathChanged("library".into()),
            &fake,
        );
        step(&mut state, Message::ContinueSetup, &fake);
        let generation = state.library_generation;
        step(
            &mut state,
            Message::LibraryValidated {
                generation,
                result: Err("Not a library".into()),
            },
            &fake,
        );
        assert!(!state.setup_busy);
        assert!(fake.library_saves.lock().unwrap().is_empty());
        step(&mut state, Message::ContinueSetup, &fake);
        let generation = state.library_generation;
        step(
            &mut state,
            Message::LibraryValidated {
                generation,
                result: Ok("library".into()),
            },
            &fake,
        );
        assert_eq!(
            *fake.library_saves.lock().unwrap(),
            [PathBuf::from("library")]
        );
        // Editing and repeated Continue cannot race a config save.
        step(&mut state, Message::SetupPathChanged("other".into()), &fake);
        step(&mut state, Message::ContinueSetup, &fake);
        assert_eq!(state.setup_path, "library");
        step(
            &mut state,
            Message::LibrarySaved {
                generation,
                result: Err("Disk full".into()),
            },
            &fake,
        );
        assert_eq!(state.setup_error.as_deref(), Some("Disk full"));
        assert!(fake.requests.lock().unwrap().is_empty());
        step(&mut state, Message::ContinueSetup, &fake);
        let generation = state.library_generation;
        step(
            &mut state,
            Message::LibraryValidated {
                generation,
                result: Ok("library".into()),
            },
            &fake,
        );
        step(
            &mut state,
            Message::LibrarySaved {
                generation,
                result: Ok("library".into()),
            },
            &fake,
        );
        assert_eq!(state.library, Some("library".into()));
        assert_eq!(state.scan_status, ScanStatus::Running);
    }
    #[test]
    fn scan_success_warnings_failure_and_cancellation() {
        let mut state = State::default();
        let fake = Fake::default();
        library_reply(&mut state, &fake, Ok(Some("library".into())));
        let id = state.active_scan.unwrap();
        fake.updates.lock().unwrap().push(scan_reply(
            id,
            Ok(rulibre_core::scanner::ScanReport {
                books: vec![],
                warnings: vec![rulibre_core::Error::unavailable("read book", None)],
            }),
            JobState::Succeeded,
        ));
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.scan_status, ScanStatus::Ready);
        assert_eq!(state.scan_warnings.len(), 1);
        assert!(!state.results.contains_key(&id));
        step(&mut state, Message::Rescan, &fake);
        let id = state.active_scan.unwrap();
        fake.updates.lock().unwrap().push(scan_reply(
            id,
            Err("read library root: permission denied".into()),
            JobState::Failed,
        ));
        step(&mut state, Message::Tick, &fake);
        assert!(
            matches!(&state.scan_status, ScanStatus::Failed(error) if error.contains("permission denied"))
        );
        step(&mut state, Message::Rescan, &fake);
        let id = state.active_scan.unwrap();
        step(&mut state, Message::Cancel(id), &fake);
        assert_eq!(state.scan_status, ScanStatus::Cancelled);
        assert_eq!(*fake.cancelled.lock().unwrap(), [id]);
        fake.updates.lock().unwrap().push(scan_reply(
            id,
            Ok(rulibre_core::scanner::ScanReport {
                books: vec![],
                warnings: vec![],
            }),
            JobState::Succeeded,
        ));
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.scan_status, ScanStatus::Cancelled);
    }
    #[test]
    fn switching_libraries_ignores_old_scan_and_startup_responses() {
        let mut state = State::default();
        let fake = Fake::default();
        library_reply(&mut state, &fake, Ok(Some("old".into())));
        let old = state.active_scan.unwrap();
        let stale_generation = state.library_generation;
        step(&mut state, Message::ChangeLibrary, &fake);
        assert_eq!(*fake.cancelled.lock().unwrap(), [old]);
        step(
            &mut state,
            Message::LibraryLoaded {
                generation: stale_generation,
                result: Ok(Some("stale".into())),
            },
            &fake,
        );
        assert_eq!(state.screen, Screen::Setup);
        step(&mut state, Message::SetupPathChanged("new".into()), &fake);
        step(&mut state, Message::ContinueSetup, &fake);
        let generation = state.library_generation;
        step(
            &mut state,
            Message::LibraryValidated {
                generation,
                result: Ok("new".into()),
            },
            &fake,
        );
        step(
            &mut state,
            Message::LibrarySaved {
                generation,
                result: Ok("new".into()),
            },
            &fake,
        );
        let new = state.active_scan.unwrap();
        fake.updates.lock().unwrap().push(scan_reply(
            old,
            Err("old failure".into()),
            JobState::Failed,
        ));
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.active_scan, Some(new));
        assert_eq!(state.scan_status, ScanStatus::Running);
        assert!(state.notices.is_empty());
        fake.updates.lock().unwrap().push(scan_reply(
            new,
            Ok(rulibre_core::scanner::ScanReport {
                books: vec![],
                warnings: vec![],
            }),
            JobState::Succeeded,
        ));
        step(&mut state, Message::Tick, &fake);
        assert_eq!(state.scan_status, ScanStatus::Ready);
        assert_eq!(state.library, Some("new".into()));
    }
    #[test]
    fn setup_picker_cancel_and_stale_result() {
        let mut state = State::default();
        let fake = Fake::default();
        step(&mut state, Message::SetupPathChanged("typed".into()), &fake);
        step(&mut state, Message::SetupBrowse, &fake);
        let generation = state.library_generation;
        step(
            &mut state,
            Message::SetupFolderPicked {
                generation,
                result: Ok(None),
            },
            &fake,
        );
        assert_eq!(state.setup_path, "typed");
        step(&mut state, Message::SetupBrowse, &fake);
        let generation = state.library_generation;
        step(&mut state, Message::ChangeLibrary, &fake);
        step(
            &mut state,
            Message::SetupFolderPicked {
                generation,
                result: Ok(Some("stale".into())),
            },
            &fake,
        );
        assert_ne!(state.setup_path, "stale");
        assert!(!state.setup_busy);
    }
    #[test]
    fn closing_during_scan_stops_services_and_ignores_async_replies() {
        let mut state = State::default();
        let fake = Fake::default();
        library_reply(&mut state, &fake, Ok(Some("library".into())));
        let generation = state.library_generation;
        step(
            &mut state,
            Message::Close(iced::window::Id::unique()),
            &fake,
        );
        assert!(state.closing);
        assert!(*fake.stopped.lock().unwrap());
        step(
            &mut state,
            Message::LibraryLoaded {
                generation,
                result: Err("late load failure".into()),
            },
            &fake,
        );
        step(
            &mut state,
            Message::LibrarySaved {
                generation,
                result: Ok("other".into()),
            },
            &fake,
        );
        assert_eq!(state.library, Some("library".into()));
        assert!(state.setup_error.is_none());
        assert_eq!(fake.requests.lock().unwrap().len(), 1);
    }
    fn key_event(
        key: iced::keyboard::Key,
        code: iced::keyboard::key::Code,
        modifiers: iced::keyboard::Modifiers,
        repeat: bool,
    ) -> Message {
        Message::Keyboard(iced::keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: iced::keyboard::key::Physical::Code(code),
            location: iced::keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat,
        })
    }
    fn press(state: &mut State, fake: &Fake, key: &str, code: iced::keyboard::key::Code) {
        press_with(state, fake, key, code, iced::keyboard::Modifiers::empty());
    }
    fn press_with(
        state: &mut State,
        fake: &Fake,
        key: &str,
        code: iced::keyboard::key::Code,
        modifiers: iced::keyboard::Modifiers,
    ) {
        let key = iced::keyboard::Key::Character(key.into());
        step(state, key_event(key, code, modifiers, false), fake);
    }
    fn press_named(
        state: &mut State,
        fake: &Fake,
        named: iced::keyboard::key::Named,
        modifiers: iced::keyboard::Modifiers,
    ) {
        let key = iced::keyboard::Key::Named(named);
        step(
            state,
            key_event(key, iced::keyboard::key::Code::Escape, modifiers, false),
            fake,
        );
    }
    fn library_state(count: u64) -> State {
        let books: Vec<_> = (0..count)
            .map(|id| Book {
                id: BookId::Calibre(id),
                author: "Author".into(),
                title: format!("Title {id:02}"),
                files: vec![],
                path: format!("book{id}").into(),
            })
            .collect();
        let mut state = State {
            screen: Screen::Home {
                filter: Filter::All,
            },
            library: Some("library".into()),
            catalog: crate::catalog::Catalog::new(books),
            scan_status: ScanStatus::Ready,
            ..Default::default()
        };
        state.settings.view = crate::settings::ViewMode::List;
        state.settings.details_visible = false;
        state.list_height = 108.0;
        state.viewport_width = 732.0;
        state
    }
    fn selected_indices(state: &State) -> Vec<u64> {
        state
            .selection
            .selected
            .iter()
            .map(|id| match id {
                BookId::Calibre(n) => *n,
                other => panic!("unexpected id {other:?}"),
            })
            .collect()
    }
    #[test]
    fn list_keys_move_page_jump_and_extend() {
        use iced::keyboard::{Modifiers, key::Named};
        let fake = Fake::default();
        let mut state = library_state(30);
        state.list_height = state.row_height() * 3.0;
        // Left/Right mean nothing in a list; the first Down selects the first book.
        press_named(&mut state, &fake, Named::ArrowRight, Modifiers::empty());
        assert!(state.selection.selected.is_empty());
        press_named(&mut state, &fake, Named::ArrowDown, Modifiers::empty());
        assert_eq!(selected_indices(&state), [0]);
        assert!(state.keyboard_focus);
        // Details open as for any selection; a page is the three visible rows.
        assert!(state.visible_settings().details_visible);
        press_named(&mut state, &fake, Named::PageDown, Modifiers::empty());
        assert_eq!(selected_indices(&state), [3]);
        press_named(&mut state, &fake, Named::PageDown, Modifiers::SHIFT);
        assert_eq!(selected_indices(&state), [3, 4, 5, 6]);
        press_named(&mut state, &fake, Named::End, Modifiers::empty());
        assert_eq!(selected_indices(&state), [29]);
        assert!(state.list_offset > 0.0);
        press_named(&mut state, &fake, Named::Home, Modifiers::SHIFT);
        assert_eq!(selected_indices(&state).len(), 30);
        press_named(&mut state, &fake, Named::PageUp, Modifiers::empty());
        assert_eq!(selected_indices(&state), [0]);
        assert_eq!(state.list_offset, 0.0);
        // Ctrl+A selects everything; a click drops the keyboard outline.
        press_with(
            &mut state,
            &fake,
            "a",
            iced::keyboard::key::Code::KeyA,
            Modifiers::CTRL,
        );
        assert_eq!(selected_indices(&state).len(), 30);
        step(&mut state, Message::ResetModifiers, &fake);
        step(&mut state, Message::BookClicked(BookId::Calibre(4)), &fake);
        assert!(!state.keyboard_focus);
        assert_eq!(selected_indices(&state), [4]);
    }
    #[test]
    fn grid_keys_move_by_card_and_by_row() {
        use iced::keyboard::{Modifiers, key::Named};
        let fake = Fake::default();
        let mut state = library_state(30);
        state.settings.view = crate::settings::ViewMode::Grid;
        // 732 px with 172 px cards and 12 px gaps: 3 columns.
        let geometry = crate::widgets::book_grid::Geometry::for_state(&state);
        assert_eq!(geometry.columns(state.viewport_width), 3);
        state.list_height = geometry.row_height * 2.0;
        press_named(&mut state, &fake, Named::ArrowRight, Modifiers::empty());
        assert_eq!(selected_indices(&state), [0]);
        press_named(&mut state, &fake, Named::ArrowRight, Modifiers::empty());
        press_named(&mut state, &fake, Named::ArrowDown, Modifiers::empty());
        assert_eq!(selected_indices(&state), [4]);
        press_named(&mut state, &fake, Named::ArrowLeft, Modifiers::empty());
        assert_eq!(selected_indices(&state), [3]);
        press_named(&mut state, &fake, Named::PageDown, Modifiers::empty());
        assert_eq!(selected_indices(&state), [9]);
        press_named(&mut state, &fake, Named::ArrowUp, Modifiers::SHIFT);
        assert_eq!(selected_indices(&state), [6, 7, 8, 9]);
        press_named(&mut state, &fake, Named::End, Modifiers::empty());
        assert_eq!(selected_indices(&state), [29]);
        assert!(state.list_offset > 0.0);
    }
    #[test]
    fn slash_opens_search_and_escape_returns_to_the_same_view() {
        let fake = Fake::default();
        let mut state = library_state(5);
        state.screen = Screen::Home {
            filter: Filter::Format(Format::Epub),
        };
        press(&mut state, &fake, "/", iced::keyboard::key::Code::Slash);
        assert_eq!(state.screen, Screen::Search);
        // Already in Search: "/" only refocuses the field.
        press(&mut state, &fake, "/", iced::keyboard::key::Code::Slash);
        assert_eq!(state.screen, Screen::Search);
        press_named(
            &mut state,
            &fake,
            iced::keyboard::key::Named::Escape,
            iced::keyboard::Modifiers::empty(),
        );
        assert_eq!(
            state.screen,
            Screen::Home {
                filter: Filter::Format(Format::Epub)
            }
        );
        // Without a remembered view Esc goes Home.
        step(&mut state, Message::Navigate(Screen::Jobs), &fake);
        step(&mut state, Message::Navigate(Screen::Search), &fake);
        press_named(
            &mut state,
            &fake,
            iced::keyboard::key::Named::Escape,
            iced::keyboard::Modifiers::empty(),
        );
        assert_eq!(
            state.screen,
            Screen::Home {
                filter: Filter::All
            }
        );
    }
    #[test]
    fn escape_closes_the_innermost_layer_first() {
        use iced::keyboard::{Modifiers, key::Named};
        let fake = Fake::default();
        let mut state = library_state(5);
        step(&mut state, Message::BookClicked(BookId::Calibre(1)), &fake);
        state.convert_menu = true;
        state.show_warnings = true;
        state.reader_picker.open = true;
        state.export_prompt = true;
        state.conversion_export = Some("export".into());
        for expected in 0..5 {
            press_named(&mut state, &fake, Named::Escape, Modifiers::empty());
            assert_eq!(state.export_prompt, expected < 0, "step {expected}");
            assert_eq!(state.reader_picker.open, expected < 1, "step {expected}");
            assert_eq!(state.convert_menu, expected < 2, "step {expected}");
            assert_eq!(state.show_warnings, expected < 3, "step {expected}");
            assert_eq!(state.selection.selected.is_empty(), expected >= 4);
        }
        // Nothing left: a further Esc is harmless.
        press_named(&mut state, &fake, Named::Escape, Modifiers::empty());
        assert!(state.selection.selected.is_empty());
    }
    #[test]
    fn send_key_explains_what_is_missing_and_ignores_repeats() {
        use iced::keyboard::Modifiers;
        let fake = Fake::default();
        let mut state = library_state(3);
        let t = || iced::keyboard::Key::Character("t".into());
        step(
            &mut state,
            key_event(
                t(),
                iced::keyboard::key::Code::KeyT,
                Modifiers::empty(),
                false,
            ),
            &fake,
        );
        assert_eq!(state.toasts.len(), 1);
        assert!(state.toasts[0].message.contains("Select the books to send"));
        // The same hint is not stacked.
        press(&mut state, &fake, "t", iced::keyboard::key::Code::KeyT);
        assert_eq!(state.toasts.len(), 1);
        step(&mut state, Message::BookClicked(BookId::Calibre(1)), &fake);
        press(&mut state, &fake, "t", iced::keyboard::key::Code::KeyT);
        assert_eq!(state.toasts.len(), 2);
        assert!(state.toasts[1].message.contains("Connect a reader"));
        let reader = fixture_reader("a", "a");
        state.readers.selected_id = Some(reader.id.clone());
        state.readers.devices.push(reader.clone());
        // Held key: repeats are dropped, the first press sends once. Selecting
        // the book already queued a metadata read.
        let baseline = fake.requests.lock().unwrap().len();
        step(
            &mut state,
            key_event(
                t(),
                iced::keyboard::key::Code::KeyT,
                Modifiers::empty(),
                true,
            ),
            &fake,
        );
        assert_eq!(fake.requests.lock().unwrap().len(), baseline);
        // Cyrillic layout: the T key types "е".
        press(&mut state, &fake, "е", iced::keyboard::key::Code::KeyT);
        assert_eq!(fake.requests.lock().unwrap().len(), baseline + 1);
        state.readers.devices[0].read_only = true;
        press(&mut state, &fake, "t", iced::keyboard::key::Code::KeyT);
        assert_eq!(fake.requests.lock().unwrap().len(), baseline + 1);
        assert!(state.toasts.iter().any(|t| t.message.contains("read-only")));
    }
    #[test]
    fn convert_key_opens_the_details_pane_and_menu() {
        let fake = Fake::default();
        let mut state = library_state(3);
        press(&mut state, &fake, "c", iced::keyboard::key::Code::KeyC);
        assert!(!state.convert_menu);
        assert!(state.toasts[0].message.contains("Select a book"));
        step(&mut state, Message::BookClicked(BookId::Calibre(0)), &fake);
        state.settings.details_visible = false;
        state.convert_menu = false;
        press(&mut state, &fake, "c", iced::keyboard::key::Code::KeyC);
        assert!(state.visible_settings().details_visible);
        assert!(state.convert_menu);
        state.settings = state.visible_settings().clone();
        state.requested_settings = None;
        press(&mut state, &fake, "с", iced::keyboard::key::Code::KeyC);
        assert!(!state.convert_menu);
        // Convert needs the details pane: not offered in Search.
        step(&mut state, Message::Navigate(Screen::Search), &fake);
        press(&mut state, &fake, "c", iced::keyboard::key::Code::KeyC);
        assert!(!state.convert_menu);
    }
    #[test]
    fn enter_opens_details_and_search_enter_reveals_in_library() {
        use iced::keyboard::{Modifiers, key::Named};
        let fake = Fake::default();
        let mut state = library_state(40);
        // Enter with nothing selected selects the first book and shows it.
        press_named(&mut state, &fake, Named::Enter, Modifiers::empty());
        assert_eq!(selected_indices(&state), [0]);
        assert!(state.visible_settings().details_visible);
        // In Search, Enter on a selected result shows it in the library.
        step(&mut state, Message::Navigate(Screen::Search), &fake);
        state.search_order = vec![BookId::Calibre(35), BookId::Calibre(7)];
        press_named(&mut state, &fake, Named::Enter, Modifiers::empty());
        assert_eq!(selected_indices(&state), [35]);
        assert_eq!(state.screen, Screen::Search);
        press_named(&mut state, &fake, Named::Enter, Modifiers::empty());
        assert_eq!(
            state.screen,
            Screen::Home {
                filter: Filter::All
            }
        );
        assert_eq!(selected_indices(&state), [35]);
        assert!(state.list_offset > 0.0);
        // The field's own Enter behaves the same.
        step(&mut state, Message::Navigate(Screen::Search), &fake);
        step(&mut state, Message::SearchSubmit, &fake);
        assert_eq!(state.screen, Screen::Search);
        step(&mut state, Message::SearchSubmit, &fake);
        assert_eq!(
            state.screen,
            Screen::Home {
                filter: Filter::All
            }
        );
    }
    #[test]
    fn shortcuts_are_ignored_in_setup_overlays_and_after_close() {
        use iced::keyboard::{Modifiers, key::Code};
        let fake = Fake::default();
        let mut state = State::default();
        assert_eq!(state.screen, Screen::Setup);
        press(&mut state, &fake, "/", Code::Slash);
        press_with(&mut state, &fake, "b", Code::KeyB, Modifiers::CTRL);
        assert_eq!(state.screen, Screen::Setup);
        assert!(fake.saves.lock().unwrap().is_empty());
        let mut state = library_state(3);
        state.reader_picker.open = true;
        press(&mut state, &fake, "/", Code::Slash);
        assert_eq!(
            state.screen,
            Screen::Home {
                filter: Filter::All
            }
        );
        state.reader_picker.open = false;
        step(
            &mut state,
            Message::Close(iced::window::Id::unique()),
            &fake,
        );
        press(&mut state, &fake, "/", Code::Slash);
        assert_ne!(state.screen, Screen::Search);
    }
    #[test]
    fn ctrl_b_and_ctrl_i_toggle_panes_and_persist() {
        use iced::keyboard::{Modifiers, key::Code};
        let fake = Fake::default();
        let mut state = library_state(3);
        assert!(state.visible_settings().sidebar_visible);
        press_with(&mut state, &fake, "b", Code::KeyB, Modifiers::CTRL);
        assert!(!state.visible_settings().sidebar_visible);
        assert!(!fake.saves.lock().unwrap().last().unwrap().sidebar_visible);
        state.settings = state.visible_settings().clone();
        state.requested_settings = None;
        press_with(&mut state, &fake, "и", Code::KeyB, Modifiers::LOGO);
        assert!(state.visible_settings().sidebar_visible);
        state.settings = state.visible_settings().clone();
        state.requested_settings = None;
        state.settings.details_visible = true;
        press_with(&mut state, &fake, "i", Code::KeyI, Modifiers::CTRL);
        assert!(!state.visible_settings().details_visible);
        // Not on Settings, which has no details pane.
        state.settings = state.visible_settings().clone();
        state.requested_settings = None;
        state.screen = Screen::Settings;
        press_with(&mut state, &fake, "i", Code::KeyI, Modifiers::CTRL);
        assert!(!state.visible_settings().details_visible);
    }
}
