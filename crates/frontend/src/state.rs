//! Application state and its mirror in the address bar.

use leptos::prelude::*;
use shared::{DEFAULT_PER_PAGE, MediaCategory, MediaItem, MediaPage, SortOrder};
use wasm_bindgen_futures::spawn_local;

use crate::api;

/// Everything that decides which files are shown. Kept in the URL so a view can
/// be reloaded or shared.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub page: u32,
    pub per_page: u32,
    pub kind: Option<MediaCategory>,
    pub sort: SortOrder,
    pub search: String,
}

impl Default for Query {
    fn default() -> Self {
        Query { page: 1, per_page: DEFAULT_PER_PAGE, kind: None, sort: SortOrder::default(), search: String::new() }
    }
}

impl Query {
    /// Reads the current `?page=…&kind=…` back into a `Query`.
    pub fn from_location() -> Self {
        let mut query = Query::default();
        let Some(search) = web_sys::window().and_then(|w| w.location().search().ok()) else {
            return query;
        };
        let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) else {
            return query;
        };

        if let Some(page) = params.get("page").and_then(|v| v.parse().ok()) {
            query.page = u32::max(page, 1);
        }
        if let Some(per_page) = params.get("per_page").and_then(|v| v.parse().ok()) {
            query.per_page = u32::clamp(per_page, 1, shared::MAX_PER_PAGE);
        }
        query.kind = params.get("kind").as_deref().and_then(MediaCategory::parse);
        query.sort = params.get("sort").as_deref().and_then(SortOrder::parse).unwrap_or_default();
        query.search = params.get("q").unwrap_or_default();

        query
    }

    /// The query string the backend expects, also used for the address bar.
    pub fn to_search(&self) -> String {
        let mut parts = vec![format!("page={}", self.page), format!("per_page={}", self.per_page)];
        if let Some(kind) = self.kind {
            parts.push(format!("kind={}", kind.as_str()));
        }
        if self.sort != SortOrder::default() {
            parts.push(format!("sort={}", self.sort.as_str()));
        }
        if !self.search.is_empty() {
            parts.push(format!("q={}", shared::encode_path_segment(&self.search)));
        }
        parts.join("&")
    }

    /// Replaces the address bar without adding a history entry — the gallery is
    /// one page, so every filter change piling up in the back button is noise.
    pub fn sync_location(&self) {
        let Some(window) = web_sys::window() else { return };
        let url = format!("?{}", self.to_search());
        let _ = window
            .history()
            .map(|history| history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url)));
    }
}

/// How far the in-flight upload has got, in bytes sent out of bytes total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadProgress {
    pub sent: u64,
    pub total: u64,
}

impl UploadProgress {
    /// Fraction of the body sent, 0.0 to 1.0.
    pub fn fraction(&self) -> f64 {
        if self.total == 0 { 0.0 } else { (self.sent as f64 / self.total as f64).clamp(0.0, 1.0) }
    }

    pub fn percent(&self) -> u32 {
        (self.fraction() * 100.0).round() as u32
    }

    /// True once the body is out and the server is still writing the files.
    pub fn finishing(&self) -> bool {
        self.total > 0 && self.sent >= self.total
    }
}

/// Signals shared by every component, handed down through context.
#[derive(Clone, Copy)]
pub struct AppState {
    pub query: RwSignal<Query>,
    pub page: RwSignal<Option<MediaPage>>,
    pub loading: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    /// Index into the current page's items, when the lightbox is open.
    pub viewing: RwSignal<Option<usize>>,
    pub uploading: RwSignal<bool>,
    /// `None` until the browser reports the first chunk of the upload.
    pub upload_progress: RwSignal<Option<UploadProgress>>,
    pub upload_note: RwSignal<Option<Result<String, String>>>,
    /// Bumped to force a refetch of the current query, e.g. after an upload.
    pub reload: RwSignal<u32>,
}

impl AppState {
    pub fn new() -> Self {
        AppState {
            query: RwSignal::new(Query::from_location()),
            page: RwSignal::new(None),
            loading: RwSignal::new(true),
            error: RwSignal::new(None),
            viewing: RwSignal::new(None),
            uploading: RwSignal::new(false),
            upload_progress: RwSignal::new(None),
            upload_note: RwSignal::new(None),
            reload: RwSignal::new(0),
        }
    }

    pub fn get() -> Self {
        expect_context::<AppState>()
    }

    /// Applies a change to the query and resets to page 1, which is what every
    /// filter change should do.
    pub fn update_filter(&self, apply: impl FnOnce(&mut Query)) {
        self.query.update(|query| {
            apply(query);
            query.page = 1;
        });
    }

    pub fn go_to_page(&self, page: u32) {
        self.viewing.set(None);
        self.query.update(|query| query.page = page.max(1));
        if let Some(window) = web_sys::window() {
            window.scroll_to_with_x_and_y(0.0, 0.0);
        }
    }

    pub fn items(&self) -> Vec<MediaItem> {
        self.page.get().map(|page| page.items).unwrap_or_default()
    }

    pub fn refresh(&self) {
        self.reload.update(|counter| *counter += 1);
    }

    /// Asks for confirmation, deletes the file on the server, then refetches.
    pub fn delete(&self, name: String) {
        let state = *self;
        let Some(window) = web_sys::window() else { return };
        if !window.confirm_with_message(&format!("Delete {name}? This cannot be undone.")).unwrap_or(false) {
            return;
        }

        spawn_local(async move {
            if let Err(err) = api::delete_media(&name).await {
                let _ = window.alert_with_message(&format!("Could not delete {name}: {err}"));
                return;
            }

            // Drop it from the page straight away, so an open lightbox moves on
            // to the next file instead of waiting for the refetch.
            state.page.update(|page| {
                if let Some(page) = page {
                    page.items.retain(|item| item.name != name);
                }
            });
            let remaining = state.page.with_untracked(|page| page.as_ref().map_or(0, |page| page.items.len()));
            state.viewing.update(|viewing| {
                if let Some(index) = *viewing {
                    *viewing = remaining.checked_sub(1).map(|last| index.min(last));
                }
            });
            state.refresh();
        });
    }
}
