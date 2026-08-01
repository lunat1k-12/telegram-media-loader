//! Inline SVG icons.
//!
//! Drawn rather than typed: the Unicode download arrows are missing from enough
//! system fonts that they show up as tofu boxes.

use leptos::prelude::*;

#[component]
pub fn DownloadIcon() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
            <path d="M12 3v11" />
            <path d="m7 11 5 5 5-5" />
            <path d="M5 20h14" />
        </svg>
    }
}
