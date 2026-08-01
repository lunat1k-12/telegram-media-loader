//! Fullscreen lightbox: images, playable video/audio, and a download link.

use leptos::prelude::*;
use shared::{MediaCategory, MediaItem};

use crate::format;
use crate::icons::DownloadIcon;
use crate::state::AppState;

#[component]
pub fn Viewer() -> impl IntoView {
    let state = AppState::get();

    let current = move || {
        let index = state.viewing.get()?;
        state.items().into_iter().nth(index).map(|item| (index, item))
    };

    let step = move |delta: i32| {
        let items = state.items();
        let Some(index) = state.viewing.get_untracked() else { return };
        let next = index as i32 + delta;
        if next >= 0 && (next as usize) < items.len() {
            state.viewing.set(Some(next as usize));
        }
    };

    // Arrow keys page through the current page's items, Escape closes.
    window_event_listener(leptos::ev::keydown, move |ev| {
        if state.viewing.get_untracked().is_none() {
            return;
        }
        match ev.key().as_str() {
            "Escape" => state.viewing.set(None),
            "ArrowLeft" => step(-1),
            "ArrowRight" => step(1),
            _ => return,
        }
        ev.prevent_default();
    });

    move || {
        current().map(|(index, item)| {
            let count = state.items().len();
            let download_url = item.download_url();
            let name = item.name.clone();

            view! {
                <div class="viewer" on:click=move |_| state.viewing.set(None)>
                    // Clicks inside the bar and the stage must not close the
                    // overlay, so they stop propagating.
                    <div class="viewer-bar" on:click=|ev| ev.stop_propagation()>
                        <span class="title">
                            {item.name.clone()}
                            <small>
                                {format!(
                                    "{} · {} · {}",
                                    shared::format_size(item.size),
                                    format::timestamp(item.modified_ms),
                                    item.mime.clone(),
                                )}
                            </small>
                        </span>
                        <span class="stats">{format!("{} / {}", index + 1, count)}</span>
                        <a class="btn" href=download_url download=name>
                            <DownloadIcon />
                            "Download"
                        </a>
                        <button class="btn" on:click=move |_| state.viewing.set(None)>
                            "✕ Close"
                        </button>
                    </div>

                    <div class="viewer-stage" on:click=|ev| ev.stop_propagation()>
                        <button
                            class="nav-arrow"
                            disabled=index == 0
                            on:click=move |_| step(-1)
                            title="Previous (←)"
                        >
                            "‹"
                        </button>
                        {stage(&item)}
                        <button
                            class="nav-arrow"
                            disabled={ index + 1 >= count }
                            on:click=move |_| step(1)
                            title="Next (→)"
                        >
                            "›"
                        </button>
                    </div>
                </div>
            }
        })
    }
}

/// The element that actually shows the file, picked from its category.
fn stage(item: &MediaItem) -> AnyView {
    let url = item.url();

    match item.category {
        MediaCategory::Image => view! { <img src=url alt=item.name.clone() /> }.into_any(),
        MediaCategory::Video => view! {
            <video src=url controls=true autoplay=true playsinline=true></video>
        }
        .into_any(),
        MediaCategory::Audio => view! {
            <audio src=url controls=true autoplay=true></audio>
        }
        .into_any(),
        MediaCategory::Other => {
            let download_url = item.download_url();
            view! {
                <div class="fallback">
                    <span class="glyph">"📄"</span>
                    <p>"No preview for this file type."</p>
                    <a class="btn" href=download_url download=item.name.clone()>
                        <DownloadIcon />
                        "Download"
                    </a>
                </div>
            }
            .into_any()
        }
    }
}
