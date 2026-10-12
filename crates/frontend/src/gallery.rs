//! The paginated grid of cards, and the pager underneath it.

use leptos::prelude::*;
use shared::{MediaCategory, MediaItem};

use crate::format;
use crate::icons::{DownloadIcon, TrashIcon};
use crate::state::AppState;

#[component]
pub fn Gallery() -> impl IntoView {
    let state = AppState::get();

    let body = move || {
        if let Some(error) = state.error.get() {
            return view! { <div class="notice error">{error}</div> }.into_any();
        }

        let items = state.items();
        if items.is_empty() {
            // Never show "no files" while the first request is still running.
            return if state.loading.get() {
                view! { <div class="notice">"Loading…"</div> }.into_any()
            } else if state.page.get().is_some_and(|page| page.total_unfiltered == 0) {
                view! { <div class="notice">"No media yet. Send something to the bot, or drop files above."</div> }
                    .into_any()
            } else {
                view! { <div class="notice">"Nothing matches this filter."</div> }.into_any()
            };
        }

        view! {
            <div class="grid" class:busy=move || state.loading.get()>
                {items
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| view! { <Card item=item index=index /> })
                    .collect_view()}
            </div>
        }
        .into_any()
    };

    view! { <main>{body}</main> }
}

#[component]
fn Card(item: MediaItem, index: usize) -> impl IntoView {
    let state = AppState::get();
    let url = item.url();
    let download_url = item.download_url();
    let name = item.name.clone();
    let title = item.name.clone();
    let delete_name = item.name.clone();

    let preview = match item.category {
        MediaCategory::Image => view! {
            <img src=url.clone() alt=name.clone() loading="lazy" decoding="async" />
        }
        .into_any(),
        MediaCategory::Video => {
            let thumbnail_url = item.thumbnail_url();
            // Set when the backend has no thumbnail (no ffmpeg, or a file it
            // cannot decode); the browser then renders a frame itself.
            let no_thumbnail = RwSignal::new(false);
            let poster = move || {
                if no_thumbnail.get() {
                    // `#t=0.1` nudges the browser past frame zero, which is
                    // usually black; it works because the backend answers
                    // range requests.
                    view! {
                        <video src=format!("{url}#t=0.1") preload="metadata" muted=true playsinline=true></video>
                    }
                    .into_any()
                } else {
                    view! {
                        <img
                            src=thumbnail_url.clone()
                            alt=name.clone()
                            loading="lazy"
                            decoding="async"
                            on:error=move |_| no_thumbnail.set(true)
                        />
                    }
                    .into_any()
                }
            };
            view! {
                {poster}
                <span class="play" aria-hidden="true"></span>
            }
            .into_any()
        }
        MediaCategory::Audio => view! { <span class="glyph">"🎧"</span> }.into_any(),
        MediaCategory::Other => view! { <span class="glyph">"📄"</span> }.into_any(),
    };

    view! {
        <article class="card">
            <button
                class="thumb"
                title=format!("Open {title}")
                on:click=move |_| state.viewing.set(Some(index))
            >
                {preview} <span class="badge">{badge_for(&item)}</span>
            </button>
            <div class="meta">
                <span class="name" title=item.name.clone()>
                    {item.name.clone()}
                </span>
                <span class="sub">
                    <span>{shared::format_size(item.size)}</span>
                    <span>{format::short_timestamp(item.modified_ms)}</span>
                    <span class="actions">
                        <a
                            href=download_url
                            download=item.name.clone()
                            title=format!("Download {}", item.name)
                        >
                            <DownloadIcon />
                        </a>
                        <button
                            class="delete"
                            title=format!("Delete {}", item.name)
                            on:click=move |_| state.delete(delete_name.clone())
                        >
                            <TrashIcon />
                        </button>
                    </span>
                </span>
            </div>
        </article>
    }
}

/// Corner label on a thumbnail. "OTHER" says nothing useful, so uncategorised
/// files show their extension instead.
fn badge_for(item: &MediaItem) -> String {
    match item.category {
        MediaCategory::Other => match item.name.rsplit_once('.') {
            Some((_, extension)) if !extension.is_empty() && extension.len() <= 5 => extension.to_uppercase(),
            _ => "file".to_uppercase(),
        },
        category => category.as_str().to_owned(),
    }
}

#[component]
pub fn Pager() -> impl IntoView {
    let state = AppState::get();

    // Driven by the response rather than the request: the backend clamps an
    // out-of-range page, and this way the UI shows where it actually landed.
    let current = move || state.page.get().map(|page| page.page).unwrap_or(1);
    let total_pages = move || state.page.get().map(|page| page.total_pages).unwrap_or(1);

    view! {
        // Braced so the `>` is not read as the end of the tag.
        <Show when=move || { total_pages() > 1 }>
            <nav class="pager">
                <button
                    class="btn"
                    disabled=move || current() <= 1
                    on:click=move |_| state.go_to_page(current().saturating_sub(1))
                >
                    "← Prev"
                </button>

                <div class="pages">
                    {move || {
                        window_of_pages(current(), total_pages())
                            .into_iter()
                            .map(|slot| match slot {
                                Some(page) => {
                                    view! {
                                        <button
                                            class:current=move || page == current()
                                            on:click=move |_| state.go_to_page(page)
                                        >
                                            {page.to_string()}
                                        </button>
                                    }
                                        .into_any()
                                }
                                None => view! { <span class="gap">"…"</span> }.into_any(),
                            })
                            .collect_view()
                    }}
                </div>

                <button
                    class="btn"
                    disabled=move || { current() >= total_pages() }
                    on:click=move |_| state.go_to_page(current() + 1)
                >
                    "Next →"
                </button>
            </nav>
        </Show>
    }
}

/// First page, last page, and a small window around the current one; `None` is
/// rendered as an ellipsis.
fn window_of_pages(current: u32, total: u32) -> Vec<Option<u32>> {
    const SPAN: u32 = 2;

    let mut slots = Vec::new();
    let mut previous = 0;
    for page in 1..=total {
        let keep = page == 1 || page == total || page.abs_diff(current) <= SPAN;
        if !keep {
            continue;
        }
        if previous != 0 && page - previous > 1 {
            slots.push(None);
        }
        slots.push(Some(page));
        previous = page;
    }
    slots
}
