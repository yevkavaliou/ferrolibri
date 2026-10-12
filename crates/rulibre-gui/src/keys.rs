use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlElement, KeyboardEvent};

use crate::ipc;
use crate::state::{AppState, Focus, Mode};
use crate::types::Metadata;

/// Returns true if the key event originated from a text input — in which case
/// global shortcuts should be skipped (e.g. typing into the search bar).
fn target_is_input(ev: &KeyboardEvent) -> bool {
    ev.target()
        .and_then(|t| t.dyn_into::<HtmlElement>().ok())
        .map(|el| {
            let tag = el.tag_name().to_lowercase();
            tag == "input" || tag == "textarea"
        })
        .unwrap_or(false)
}

pub fn handle_global_keydown(state: AppState, ev: KeyboardEvent) {
    let mode = state.mode.get_untracked();
    if matches!(mode, Mode::Search) {
        // Search input owns its own onkeydown handler.
        return;
    }
    if target_is_input(&ev) && !matches!(mode, Mode::Setup) {
        return;
    }
    let key = ev.key();
    match mode {
        Mode::Normal => normal(state, &key, &ev),
        Mode::Detail => detail(state, &key, &ev),
        Mode::Convert => convert(state, &key, &ev),
        Mode::Search => unreachable!(),
        Mode::Setup => setup_global(state, &key, &ev),
    }
}

fn normal(state: AppState, key: &str, ev: &KeyboardEvent) {
    match key {
        "q" | "Escape" => {
            // Closing the window is delegated to Tauri's default close shortcut;
            // no-op here so unsaved state isn't lost.
        }
        "ArrowDown" | "s" => {
            state.next();
            ev.prevent_default();
        }
        "ArrowUp" | "w" => {
            state.previous();
            ev.prevent_default();
        }
        "/" => {
            state.mode.set(Mode::Search);
            ev.prevent_default();
        }
        "Enter" => open_detail(state),
        "c" => enter_convert(state),
        "t" => send_to_device(state),
        _ => {}
    }
}

fn detail(state: AppState, key: &str, ev: &KeyboardEvent) {
    match key {
        "Escape" | "q" => close_detail(state),
        "ArrowLeft" | "a" => state.focus.set(Focus::Table),
        "ArrowRight" | "d" => state.focus.set(Focus::Detail),
        "ArrowDown" | "s" => match state.focus.get_untracked() {
            Focus::Table => {
                state.next();
                refresh_detail(state);
                ev.prevent_default();
            }
            Focus::Detail => {} // browser handles scroll
        },
        "ArrowUp" | "w" => match state.focus.get_untracked() {
            Focus::Table => {
                state.previous();
                refresh_detail(state);
                ev.prevent_default();
            }
            Focus::Detail => {}
        },
        "c" => enter_convert(state),
        "t" => send_to_device(state),
        "Enter" => match state.focus.get_untracked() {
            Focus::Table => open_detail(state),
            Focus::Detail => close_detail(state),
        },
        _ => {}
    }
}

fn convert(state: AppState, key: &str, ev: &KeyboardEvent) {
    match key {
        "Escape" | "q" => {
            state.convert_generation.update(|n| *n += 1);
            state.conversion_busy.set(false);
            if let Some(id) = state.active_conversion.get_untracked() {
                state.active_conversion.set(None);
                spawn_local(async move {
                    if let Err(error) = ipc::cancel_job(id).await {
                        state.notify(Err(error));
                    }
                });
            }
            state.mode.set(Mode::Normal);
            state.convert_targets.set(Vec::new());
            state.convert_message.set(None);
        }
        "ArrowDown" | "s" => {
            let targets = state.convert_targets.get();
            if !targets.is_empty() {
                let cur = state.convert_selected.get();
                state.convert_selected.set((cur + 1) % targets.len());
            }
            ev.prevent_default();
        }
        "ArrowUp" | "w" => {
            let targets = state.convert_targets.get();
            if !targets.is_empty() {
                let cur = state.convert_selected.get();
                let next = if cur == 0 { targets.len() - 1 } else { cur - 1 };
                state.convert_selected.set(next);
            }
            ev.prevent_default();
        }
        "Enter" => run_convert(state),
        _ => {}
    }
}

fn setup_global(_state: AppState, _key: &str, _ev: &KeyboardEvent) {
    // Setup mode is fully handled inside the input element.
}

pub fn open_detail(state: AppState) {
    if state.selected_book().is_none() {
        return;
    }
    state.mode.set(Mode::Detail);
    state.focus.set(Focus::Table);
    refresh_detail(state);
}

pub fn refresh_detail(state: AppState) {
    state.metadata_generation.update(|n| *n += 1);
    let generation = state.metadata_generation.get_untracked();
    state.detail.set(None);
    let Some(book) = state.selected_book() else {
        return;
    };
    spawn_local(async move {
        let result = ipc::parse_metadata(book.path).await;
        let selected = state.selected_book();
        if !crate::response_guard::accepts(
            generation,
            state.metadata_generation.get_untracked(),
            &book.id,
            selected.as_ref().map(|b| &b.id),
            state.mode.get_untracked() == Mode::Detail,
        ) {
            return;
        }
        match result {
            Ok(meta) => state.detail.set(meta.or(Some(Metadata::default()))),
            Err(error) => state.notify(Err(error)),
        }
    });
}

pub fn close_detail(state: AppState) {
    state.metadata_generation.update(|n| *n += 1);
    state.mode.set(Mode::Normal);
    state.focus.set(Focus::Table);
    state.detail.set(None);
}

pub fn enter_convert(state: AppState) {
    let Some(book) = state.selected_book() else {
        return;
    };
    state.convert_generation.update(|n| *n += 1);
    let generation = state.convert_generation.get_untracked();
    state.convert_targets.set(Vec::new());
    state.convert_message.set(None);
    state.mode.set(Mode::Convert);
    spawn_local(async move {
        let backends = match ipc::available_backends().await {
            Ok(value) => value,
            Err(error) => {
                state.notify(Err(error));
                return;
            }
        };
        if generation != state.convert_generation.get_untracked()
            || state.selected_book().as_ref().map(|b| &b.id) != Some(&book.id)
            || state.mode.get_untracked() != Mode::Convert
        {
            return;
        }
        let k = backends.iter().any(|b| {
            b.kind == crate::types::BackendKind::Kepubify
                && b.status == crate::types::BackendStatus::Ready
        });
        let c = backends.iter().any(|b| {
            b.kind == crate::types::BackendKind::EbookConvert
                && b.status == crate::types::BackendStatus::Ready
        });
        state.backends.set((k, c));
        if !k && !c {
            state.convert_targets.set(Vec::new());
            state.convert_message.set(Some(Err(backends
                .iter()
                .map(|b| format!("{}: {}", b.kind.program(), b.diagnostic))
                .collect::<Vec<_>>()
                .join("; "))));
            state.mode.set(Mode::Convert);
            return;
        }
        let targets = match ipc::target_formats(book.files.clone(), backends).await {
            Ok(value) => value,
            Err(error) => {
                state.notify(Err(error));
                return;
            }
        };
        if generation != state.convert_generation.get_untracked()
            || state.selected_book().as_ref().map(|b| &b.id) != Some(&book.id)
            || state.mode.get_untracked() != Mode::Convert
        {
            return;
        }
        if targets.is_empty() {
            state.convert_targets.set(Vec::new());
            state
                .convert_message
                .set(Some(Err("No formats to convert to".into())));
            state.mode.set(Mode::Convert);
            return;
        }
        state.convert_targets.set(targets);
        state.convert_selected.set(0);
        state.convert_message.set(None);
        state.mode.set(Mode::Convert);
    });
}

pub fn run_convert(state: AppState) {
    if state.conversion_busy.get_untracked() {
        return;
    }
    let targets = state.convert_targets.get();
    let Some((target, _)) = targets.get(state.convert_selected.get()).cloned() else {
        return;
    };
    let Some(book) = state.selected_book() else {
        return;
    };
    state.convert_generation.update(|n| *n += 1);
    let generation = state.convert_generation.get_untracked();
    state.conversion_busy.set(true);
    state
        .convert_message
        .set(Some(Ok("Waiting for conversion".into())));
    spawn_local(async move {
        let result = ipc::convert_book(
            book.path,
            book.files,
            target,
            move |id| {
                if generation == state.convert_generation.get_untracked() {
                    state.active_conversion.set(Some(id));
                } else {
                    spawn_local(async move {
                        if let Err(error) = ipc::cancel_job(id).await {
                            state.notify(Err(error));
                        }
                    });
                }
            },
            move |event| {
                if generation == state.convert_generation.get_untracked() {
                    if let crate::types::JobEvent::Stage { stage, .. } = event {
                        state.convert_message.set(Some(Ok(format!("{stage:?}"))));
                    }
                }
            },
        )
        .await;
        if generation != state.convert_generation.get_untracked() {
            return;
        }
        state.conversion_busy.set(false);
        state.active_conversion.set(None);
        let selected = state.selected_book();
        let applies = crate::response_guard::accepts(
            generation,
            state.convert_generation.get_untracked(),
            &book.id,
            selected.as_ref().map(|b| &b.id),
            state.mode.get_untracked() == Mode::Convert,
        );
        let message = result.map(|ok| ok.message);
        if applies {
            state.convert_message.set(Some(message.clone()));
            if message.is_ok() {
                state.convert_targets.set(Vec::new());
                state.mode.set(Mode::Normal);
            }
        }
        state.notify(
            message
                .map(|message| format!("{}: {message}", book.title))
                .map_err(|error| format!("{}: {error}", book.title)),
        );
    });
}

pub fn send_to_device(state: AppState) {
    let Some(book) = state.selected_book() else {
        return;
    };
    spawn_local(async move {
        let result = ipc::send_to_device(book.path, book.author, move |event| {
            if let crate::types::JobEvent::CopyProgress {
                completed, total, ..
            } = event
            {
                state.notify(Ok(format!("Copied {completed}/{total} bytes")));
            }
        })
        .await;
        state.notify(
            result
                .map(|message| format!("{}: {message}", book.title))
                .map_err(|error| format!("{}: {error}", book.title)),
        );
    });
}
