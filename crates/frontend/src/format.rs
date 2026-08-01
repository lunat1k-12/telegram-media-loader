//! Small formatting helpers for values that come off the wire as numbers.

use wasm_bindgen::JsValue;

/// `1754120000000` → `2 Aug 2026, 09:27`, in the viewer's own timezone.
///
/// Built from the `Date` accessors rather than `toLocaleString` so the output
/// is the same everywhere and stays short enough for a card.
pub fn timestamp(ms: u64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

    let date = js_sys::Date::new(&JsValue::from_f64(ms as f64));
    let month = MONTHS.get(date.get_month() as usize).copied().unwrap_or("?");

    format!("{} {} {}, {:02}:{:02}", date.get_date(), month, date.get_full_year(), date.get_hours(), date.get_minutes())
}

/// Shorter variant for the card footer, where the year is usually redundant.
pub fn short_timestamp(ms: u64) -> String {
    let full = timestamp(ms);
    full.split_once(", ").map(|(date, _)| date.to_owned()).unwrap_or(full)
}
