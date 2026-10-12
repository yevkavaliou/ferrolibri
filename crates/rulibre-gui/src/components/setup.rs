use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::ipc;
use crate::state::{AppState, Mode};

#[component]
pub fn Setup() -> impl IntoView {
    let state = expect_context::<AppState>();

    let on_input = move |ev| {
        let value = event_target_value(&ev);
        state.setup_input.set(value);
        state.setup_error.set(String::new());
    };

    let submit = move || {
        if state.setup_busy.get_untracked() {
            return;
        }
        state.setup_busy.set(true);
        let path = state.setup_input.get();
        spawn_local(async move {
            match ipc::validate_library(path.clone()).await {
                Ok(sanitized) => {
                    if let Err(err) = ipc::save_config(sanitized.clone()).await {
                        state.setup_busy.set(false);
                        state.setup_error.set(err);
                        return;
                    }
                    let report = match ipc::scan_library(sanitized).await {
                        Ok(report) => report,
                        Err(e) => {
                            state.setup_busy.set(false);
                            state.setup_error.set(e);
                            return;
                        }
                    };
                    if !report.warnings.is_empty() {
                        state.notify(Err(report.warnings.join("; ")));
                    }
                    let books = report.books;
                    let has_books = !books.is_empty();
                    state.all_books.set(books);
                    state
                        .selected_idx
                        .set(if has_books { Some(0) } else { None });
                    state.mode.set(Mode::Normal);
                    state.setup_busy.set(false);
                }
                Err(err) => {
                    state.setup_busy.set(false);
                    state.setup_error.set(err);
                }
            }
        });
    };

    let on_click = move |_: leptos::ev::MouseEvent| submit();
    let on_key = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" {
            submit();
        }
    };

    view! {
        <div class="modal-scrim">
            <div class="modal" role="dialog">
                <h2>"Enter Calibre library path"</h2>
                <input
                    class="modal-input"
                    type="text"
                    autofocus="true"
                    placeholder="~/Calibre Library"
                    prop:value=move || state.setup_input.get()
                    on:input=on_input
                    on:keydown=on_key
                />
                <div class="modal-error">{move || state.setup_error.get()}</div>
                <div class="modal-hint">
                    <span><kbd>"Enter"</kbd>" confirm"</span>
                </div>
                <div class="modal-actions">
                    <button on:click=on_click disabled=move || state.setup_busy.get()>"Open library"</button>
                </div>
            </div>
        </div>
    }
}
