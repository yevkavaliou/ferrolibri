use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::KeyboardEvent;

use crate::components::context_menu::ContextMenu;
use crate::components::convert::Convert;
use crate::components::detail::Detail;
use crate::components::notification::Notification;
use crate::components::setup::Setup;
use crate::components::status_bar::StatusBar;
use crate::components::table::BookTable;
use crate::ipc;
use crate::keys::handle_global_keydown;
use crate::state::{AppState, Mode};

#[component]
pub fn App() -> impl IntoView {
    let state = AppState::new();
    provide_context(state);

    bootstrap(state);
    install_global_keydown(state);
    install_device_listener(state);

    let layout_class = move || match state.mode.get() {
        Mode::Detail => "layout split",
        _ => "layout",
    };

    let show_setup = move || matches!(state.mode.get(), Mode::Setup);

    view! {
        <main class="app">
            <div class=layout_class>
                <BookTable />
                <Detail />
            </div>
            <StatusBar />
            <Notification />
            <ContextMenu />
            <Convert />
            <Show when=show_setup fallback=|| view! { <></> }>
                <Setup />
            </Show>
        </main>
    }
}

fn bootstrap(state: AppState) {
    spawn_local(async move {
        match ipc::load_config().await {
            Ok(Some(config)) => {
                let report = match ipc::scan_library(config.library_path).await {
                    Ok(report) => report,
                    Err(e) => {
                        state.setup_error.set(e);
                        state.mode.set(Mode::Setup);
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
            }
            Err(e) => {
                state.setup_error.set(e);
                state.mode.set(Mode::Setup);
            }
            Ok(None) => {
                state.mode.set(Mode::Setup);
            }
        }

        match ipc::current_device().await {
            Ok(device) => state.device.set(device),
            Err(error) => state.notify(Err(error)),
        }
    });
}

fn install_global_keydown(state: AppState) {
    let window = web_sys::window().expect("window");
    let closure = wasm_bindgen::closure::Closure::<dyn FnMut(KeyboardEvent)>::new(
        move |ev: KeyboardEvent| {
            handle_global_keydown(state, ev);
        },
    );
    window
        .add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref())
        .expect("attach keydown");
    closure.forget();
}

fn install_device_listener(state: AppState) {
    ipc::listen_device_events(move |event| match event {
        Ok(event) => {
            state.device.set(event.selected_device().cloned());
            state.notify(event.message());
        }
        Err(error) => state.notify(Err(error)),
    });
}
