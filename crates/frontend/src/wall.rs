//! Full-screen picture wall at `/wall`: a grid of random images where one tile
//! at a time fades to another, in the spirit of Google Trends' TV screen.
//!
//! Configured from the address bar, e.g. `/wall?rows=3&cols=4&interval=5`.

use std::collections::HashSet;
use std::time::Duration;

use leptos::prelude::*;
use shared::{MediaCategory, MediaItem};
use wasm_bindgen_futures::spawn_local;

use crate::api;

const DEFAULT_SIZE: u32 = 3;
const MAX_SIZE: u32 = 10;
/// Seconds between two tile swaps.
const DEFAULT_INTERVAL: f64 = 4.0;
const MIN_INTERVAL: f64 = 0.5;
/// Matches the opacity transition on `.wall-layer` in styles.css; the layer
/// underneath is dropped once the new one has fully faded in.
const FADE: Duration = Duration::from_millis(1200);

/// Grid size and pace, read once from the address bar.
#[derive(Clone, Copy, Debug)]
struct WallConfig {
    rows: u32,
    cols: u32,
    interval: Duration,
}

impl WallConfig {
    fn from_location() -> Self {
        let params = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok());
        let get = |key: &str| params.as_ref().and_then(|params| params.get(key));

        let size = |key: &str| get(key).and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_SIZE).clamp(1, MAX_SIZE);
        let interval = get("interval").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_INTERVAL).max(MIN_INTERVAL);

        WallConfig { rows: size("rows"), cols: size("cols"), interval: Duration::from_secs_f64(interval) }
    }

    fn cells(&self) -> usize {
        (self.rows * self.cols) as usize
    }
}

/// One image stacked in a tile. A tile holds two while the new one fades in.
#[derive(Clone, Debug, PartialEq)]
struct Layer {
    id: u32,
    item: MediaItem,
}

#[derive(Clone, Copy)]
struct Tile {
    layers: RwSignal<Vec<Layer>>,
}

impl Tile {
    /// The image the tile is showing or fading towards.
    fn current(&self) -> Option<String> {
        self.layers.with_untracked(|layers| layers.last().map(|layer| layer.item.name.clone()))
    }
}

#[component]
pub fn Wall() -> impl IntoView {
    let config = WallConfig::from_location();
    let tiles: Vec<Tile> = (0..config.cells()).map(|_| Tile { layers: RwSignal::new(Vec::new()) }).collect();
    let next_id = StoredValue::new(0u32);
    let error = RwSignal::new(None::<String>);

    let show = move |tile: Tile, item: MediaItem| {
        let id = next_id.get_value();
        next_id.set_value(id + 1);
        tile.layers.update(|layers| layers.push(Layer { id, item }));
    };

    // Fill every tile. A folder with fewer images than tiles repeats them.
    {
        let tiles = tiles.clone();
        spawn_local(async move {
            match api::fetch_random(config.cells(), Some(MediaCategory::Image)).await {
                Ok(items) if items.is_empty() => error.set(Some("No images to show yet.".into())),
                Ok(items) => {
                    for (tile, item) in tiles.iter().zip(items.iter().cycle()) {
                        show(*tile, item.clone());
                    }
                }
                Err(err) => error.set(Some(err)),
            }
        });
    }

    // Swap one random tile per tick, to an image that is not already on the
    // wall. Asking for one more than there are tiles guarantees such an image
    // whenever the folder has one.
    {
        let tiles = tiles.clone();
        set_interval(
            move || {
                let tiles = tiles.clone();
                spawn_local(async move {
                    let Ok(items) = api::fetch_random(config.cells() + 1, Some(MediaCategory::Image)).await else {
                        return;
                    };
                    let on_wall: HashSet<String> = tiles.iter().filter_map(Tile::current).collect();
                    let Some(item) = items.into_iter().find(|item| !on_wall.contains(&item.name)) else {
                        return;
                    };
                    let index = (js_sys::Math::random() * tiles.len() as f64) as usize;
                    show(tiles[index.min(tiles.len() - 1)], item);
                });
            },
            config.interval,
        );
    }

    let style = format!(
        "grid-template-columns: repeat({}, 1fr); grid-template-rows: repeat({}, 1fr)",
        config.cols, config.rows
    );

    view! {
        <div class="wall" style=style>
            {tiles.into_iter().map(|tile| view! { <WallTile tile /> }).collect_view()}
            {move || error.get().map(|message| view! { <div class="wall-message">{message}</div> })}
        </div>
    }
}

#[component]
fn WallTile(tile: Tile) -> impl IntoView {
    view! {
        <div class="wall-tile">
            <For each=move || tile.layers.get() key=|layer| layer.id let:layer>
                <WallLayer tile layer />
            </For>
        </div>
    }
}

/// Starts transparent and fades in once the image has loaded, so a slow
/// download never blanks the tile; then retires the layers underneath it.
#[component]
fn WallLayer(tile: Tile, layer: Layer) -> impl IntoView {
    let loaded = RwSignal::new(false);
    let id = layer.id;

    let on_load = move |_| {
        loaded.set(true);
        set_timeout(move || tile.layers.update(|layers| layers.retain(|layer| layer.id >= id)), FADE);
    };
    // A broken file is dropped so the tile keeps what it had.
    let on_error = move |_| tile.layers.update(|layers| layers.retain(|layer| layer.id != id));

    view! {
        <img
            class="wall-layer"
            class:loaded=loaded
            src=layer.item.url()
            alt=layer.item.name.clone()
            on:load=on_load
            on:error=on_error
        />
    }
}
