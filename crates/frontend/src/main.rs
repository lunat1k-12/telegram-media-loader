//! Leptos client-side app for browsing `SAVE_DIR`.
//!
//! It talks to the Axum backend over `/api` and is served by that same backend
//! as static files, so there is a single origin and no CORS in production.

mod api;
mod format;
mod gallery;
mod icons;
mod state;
mod toolbar;
mod uploader;
mod viewer;

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::gallery::{Gallery, Pager};
use crate::state::AppState;
use crate::toolbar::{Masthead, Toolbar};
use crate::uploader::Uploader;
use crate::viewer::Viewer;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let state = AppState::new();
    provide_context(state);

    // Typing in the search box fires a request per keystroke, and those can come
    // back out of order; only the newest one is allowed to touch the state.
    let generation = RwSignal::new(0u32);

    // One effect owns all fetching: it re-runs whenever the query changes or
    // `refresh()` bumps the counter.
    Effect::new(move |_| {
        let query = state.query.get();
        let _ = state.reload.get();
        query.sync_location();

        let request = generation.get_untracked() + 1;
        generation.set(request);

        state.loading.set(true);
        spawn_local(async move {
            let result = api::fetch_page(&query).await;
            if generation.get_untracked() != request {
                return;
            }

            match result {
                Ok(page) => {
                    state.error.set(None);
                    state.page.set(Some(page));
                }
                Err(err) => state.error.set(Some(err)),
            }
            state.loading.set(false);
        });
    });

    view! {
        <div class="app">
            <Masthead />
            <Toolbar />
            <Uploader />
            <Gallery />
            <Pager />
            <Viewer />
        </div>
    }
}
