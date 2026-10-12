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

#[component]
pub fn TrashIcon() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
            <path d="M4 7h16" />
            <path d="M9 7V4h6v3" />
            <path d="m6 7 1 13h10l1-13" />
            <path d="M10 11v5" />
            <path d="M14 11v5" />
        </svg>
    }
}

#[component]
pub fn PlugIcon() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
            <path d="M9 3v5" />
            <path d="M15 3v5" />
            <path d="M6 8h12v3a6 6 0 0 1-12 0z" />
            <path d="M12 17v4" />
        </svg>
    }
}
