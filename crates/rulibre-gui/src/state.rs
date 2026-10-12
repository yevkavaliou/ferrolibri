use leptos::prelude::*;

use crate::types::{Book, Device, Metadata};

#[derive(Clone, Copy)]
pub struct ContextMenuData {
    pub x: i32,
    pub y: i32,
    pub book_idx: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Setup,
    Normal,
    Search,
    Detail,
    Convert,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Focus {
    Table,
    Detail,
}

#[derive(Clone, Copy)]
pub struct AppState {
    pub mode: RwSignal<Mode>,
    pub metadata_generation: RwSignal<u64>,
    pub convert_generation: RwSignal<u64>,
    pub conversion_busy: RwSignal<bool>,
    pub active_conversion: RwSignal<Option<crate::types::JobId>>,
    pub focus: RwSignal<Focus>,
    pub all_books: RwSignal<Vec<Book>>,
    pub search_query: RwSignal<String>,
    pub selected_idx: RwSignal<Option<usize>>,
    pub detail: RwSignal<Option<Metadata>>,
    pub device: RwSignal<Option<Device>>,
    pub backends: RwSignal<(bool, bool)>,
    pub convert_targets: RwSignal<Vec<(String, String)>>,
    pub convert_selected: RwSignal<usize>,
    pub convert_message: RwSignal<Option<Result<String, String>>>,
    pub setup_busy: RwSignal<bool>,
    pub setup_input: RwSignal<String>,
    pub setup_error: RwSignal<String>,
    pub notification_generation: RwSignal<u64>,
    pub notification: RwSignal<Option<Result<String, String>>>,
    pub context_menu: RwSignal<Option<ContextMenuData>>,
    pub filtered: Memo<Vec<Book>>,
}

impl AppState {
    pub fn new() -> Self {
        let all_books: RwSignal<Vec<Book>> = RwSignal::new(Vec::new());
        let search_query: RwSignal<String> = RwSignal::new(String::new());
        let filtered: Memo<Vec<Book>> = Memo::new(move |_| {
            let q = search_query.get().to_lowercase();
            let books = all_books.get();
            if q.is_empty() {
                books
            } else {
                books
                    .into_iter()
                    .filter(|b| {
                        b.author.to_lowercase().contains(&q)
                            || b.title.to_lowercase().contains(&q)
                            || b.formats.to_lowercase().contains(&q)
                    })
                    .collect()
            }
        });

        Self {
            mode: RwSignal::new(Mode::Normal),
            metadata_generation: RwSignal::new(0),
            convert_generation: RwSignal::new(0),
            conversion_busy: RwSignal::new(false),
            active_conversion: RwSignal::new(None),
            focus: RwSignal::new(Focus::Table),
            all_books,
            search_query,
            selected_idx: RwSignal::new(None),
            detail: RwSignal::new(None),
            device: RwSignal::new(None),
            backends: RwSignal::new((false, false)),
            convert_targets: RwSignal::new(Vec::new()),
            convert_selected: RwSignal::new(0),
            convert_message: RwSignal::new(None),
            setup_busy: RwSignal::new(false),
            setup_input: RwSignal::new(String::new()),
            setup_error: RwSignal::new(String::new()),
            notification_generation: RwSignal::new(0),
            notification: RwSignal::new(None),
            context_menu: RwSignal::new(None),
            filtered,
        }
    }

    pub fn selected_book(&self) -> Option<Book> {
        let idx = self.selected_idx.get()?;
        self.filtered.get().get(idx).cloned()
    }

    pub fn next(&self) {
        let books = self.filtered.get();
        if books.is_empty() {
            return;
        }
        let i = self
            .selected_idx
            .get()
            .map_or(0, |i| if i + 1 >= books.len() { 0 } else { i + 1 });
        self.selected_idx.set(Some(i));
    }

    pub fn previous(&self) {
        let books = self.filtered.get();
        if books.is_empty() {
            return;
        }
        let i = self
            .selected_idx
            .get()
            .map_or(0, |i| if i == 0 { books.len() - 1 } else { i - 1 });
        self.selected_idx.set(Some(i));
    }

    pub fn notify(&self, result: Result<String, String>) {
        self.notification.set(Some(result));
        self.notification_generation.update(|n| *n += 1);
        let generation = self.notification_generation.get_untracked();
        let current = self.notification_generation;
        let signal = self.notification;
        wasm_bindgen_futures::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(3000).await;
            if current.get_untracked() == generation {
                signal.set(None);
            }
        });
    }

    pub fn after_apply_filter(&self) {
        let books = self.filtered.get();
        if books.is_empty() {
            self.selected_idx.set(None);
        } else {
            self.selected_idx.set(Some(0));
        }
    }
}
