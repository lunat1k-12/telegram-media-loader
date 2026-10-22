//! Title bar with the folder totals and the battery/plug readout, and the
//! filter/sort/search controls.

use std::time::Duration;

use leptos::prelude::*;
use shared::{MediaCategory, PowerStatus, SortOrder};
use wasm_bindgen_futures::spawn_local;

use crate::api;
use crate::icons::PlugIcon;
use crate::state::AppState;

/// The backend re-reads the battery once a minute; polling twice as often keeps
/// the readout at most ~90 s behind.
const POWER_POLL_INTERVAL: Duration = Duration::from_secs(30);
/// At or below this charge the gauge turns red.
const LOW_BATTERY: u8 = 20;

#[component]
pub fn Masthead() -> impl IntoView {
    let state = AppState::get();

    let stats = move || {
        state.page.get().map(|page| {
            let filtered = if page.total == page.total_unfiltered {
                String::new()
            } else {
                format!(" · {} matching", page.total)
            };
            format!("{} files · {}{}", page.total_unfiltered, shared::format_size(page.total_bytes), filtered)
        })
    };

    view! {
        <header class="masthead">
            <h1>"Media library"</h1>
            <span class="stats">{stats}</span>
            <span class="spacer"></span>
            <a class="wall-link" href="/wall?rows=3&cols=3" target="_blank">"Picture wall"</a>
            <PowerIndicator />
        </header>
    }
}

/// Battery charge and smart-plug state of the machine the server runs on.
/// Renders nothing when that machine has no battery.
#[component]
fn PowerIndicator() -> impl IntoView {
    let power = RwSignal::new(PowerStatus::default());

    let poll = move || {
        spawn_local(async move {
            // A failed poll clears the readout rather than leaving a stale one.
            power.set(api::fetch_power().await.unwrap_or_default());
        });
    };
    poll();
    set_interval(poll, POWER_POLL_INTERVAL);

    move || {
        let PowerStatus { battery_percent, plug_on } = power.get();
        let percent = battery_percent?;
        let plug = match plug_on {
            Some(true) => "Plug on",
            Some(false) => "Plug off",
            None => "Plug unreachable",
        };

        Some(view! {
            <span class="power">
                <span title="Battery charge">
                    <span class="battery" class:low={ percent <= LOW_BATTERY }>
                        <span class="battery-fill" style=format!("width: {percent}%")></span>
                    </span>
                    {format!("{percent}%")}
                </span>
                <span class="plug" class:on={ plug_on == Some(true) } class:unknown=plug_on.is_none()>
                    <PlugIcon />
                    {plug}
                </span>
            </span>
        })
    }
}

#[component]
pub fn Toolbar() -> impl IntoView {
    let state = AppState::get();

    let is_active = move |kind: Option<MediaCategory>| state.query.get().kind == kind;

    view! {
        <div class="toolbar">
            <div class="filters">
                <button
                    class:active=move || is_active(None)
                    on:click=move |_| state.update_filter(|query| query.kind = None)
                >
                    "All"
                </button>
                {MediaCategory::ALL
                    .into_iter()
                    .map(|kind| {
                        view! {
                            <button
                                class:active=move || is_active(Some(kind))
                                on:click=move |_| state.update_filter(|query| query.kind = Some(kind))
                            >
                                {label_for(kind)}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>

            <div class="search">
                <input
                    type="search"
                    placeholder="Search file names…"
                    prop:value=move || state.query.get().search
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        state.update_filter(|query| query.search = value);
                    }
                />
            </div>

            <select on:change=move |ev| {
                let value = event_target_value(&ev);
                if let Some(sort) = SortOrder::parse(&value) {
                    state.update_filter(|query| query.sort = sort);
                }
            }>
                {SortOrder::ALL
                    .into_iter()
                    .map(|sort| {
                        view! {
                            <option
                                value=sort.as_str()
                                selected=move || state.query.get().sort == sort
                            >
                                {sort.label()}
                            </option>
                        }
                    })
                    .collect_view()}
            </select>

            <select on:change=move |ev| {
                if let Ok(per_page) = event_target_value(&ev).parse::<u32>() {
                    state.update_filter(|query| query.per_page = per_page);
                }
            }>
                {[12u32, 24, 48, 96]
                    .into_iter()
                    .map(|size| {
                        view! {
                            <option
                                value=size.to_string()
                                selected=move || state.query.get().per_page == size
                            >
                                {format!("{size} / page")}
                            </option>
                        }
                    })
                    .collect_view()}
            </select>
        </div>
    }
}

fn label_for(kind: MediaCategory) -> &'static str {
    match kind {
        MediaCategory::Image => "Images",
        MediaCategory::Video => "Video",
        MediaCategory::Audio => "Audio",
        MediaCategory::Other => "Files",
    }
}
