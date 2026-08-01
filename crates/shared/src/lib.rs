//! Types shared by the Axum backend and the Leptos frontend.
//!
//! Everything here is serde-serialisable and free of platform specific
//! dependencies, so the same definitions describe the JSON on both ends of the
//! wire.

use serde::{Deserialize, Serialize};

/// Default number of items per page, used when the request omits `per_page`.
pub const DEFAULT_PER_PAGE: u32 = 24;
/// Upper bound on `per_page`, so a crafted request cannot ask for the whole
/// directory at once.
pub const MAX_PER_PAGE: u32 = 200;

/// Broad media class, derived from the file extension.
///
/// The frontend picks a renderer from this (`<img>`, `<video>`, `<audio>` or a
/// plain download card), which is why the mapping lives in the shared crate
/// instead of the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaCategory {
    Image,
    Video,
    Audio,
    Other,
}

impl MediaCategory {
    /// The value used in the `kind=` query parameter and in the UI filter.
    pub fn as_str(self) -> &'static str {
        match self {
            MediaCategory::Image => "image",
            MediaCategory::Video => "video",
            MediaCategory::Audio => "audio",
            MediaCategory::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "image" => Some(MediaCategory::Image),
            "video" => Some(MediaCategory::Video),
            "audio" => Some(MediaCategory::Audio),
            "other" => Some(MediaCategory::Other),
            _ => None,
        }
    }

    pub const ALL: [MediaCategory; 4] =
        [MediaCategory::Image, MediaCategory::Video, MediaCategory::Audio, MediaCategory::Other];

    /// Classifies a file name by its extension.
    pub fn from_file_name(name: &str) -> Self {
        let extension = name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or_default().to_ascii_lowercase();

        match extension.as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "avif" | "heic" | "heif" | "svg" | "tiff" | "tif" => {
                MediaCategory::Image
            }
            "mp4" | "webm" | "mov" | "mkv" | "avi" | "m4v" | "mpeg" | "mpg" | "ogv" | "3gp" => MediaCategory::Video,
            "mp3" | "oga" | "ogg" | "opus" | "m4a" | "aac" | "wav" | "flac" | "weba" => MediaCategory::Audio,
            _ => MediaCategory::Other,
        }
    }
}

/// How a page of results is ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    /// Most recently modified first — the default, newest uploads on page 1.
    #[default]
    Newest,
    Oldest,
    NameAsc,
    NameDesc,
    Largest,
    Smallest,
}

impl SortOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            SortOrder::Newest => "newest",
            SortOrder::Oldest => "oldest",
            SortOrder::NameAsc => "name_asc",
            SortOrder::NameDesc => "name_desc",
            SortOrder::Largest => "largest",
            SortOrder::Smallest => "smallest",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "newest" => Some(SortOrder::Newest),
            "oldest" => Some(SortOrder::Oldest),
            "name_asc" => Some(SortOrder::NameAsc),
            "name_desc" => Some(SortOrder::NameDesc),
            "largest" => Some(SortOrder::Largest),
            "smallest" => Some(SortOrder::Smallest),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortOrder::Newest => "Newest first",
            SortOrder::Oldest => "Oldest first",
            SortOrder::NameAsc => "Name A→Z",
            SortOrder::NameDesc => "Name Z→A",
            SortOrder::Largest => "Largest first",
            SortOrder::Smallest => "Smallest first",
        }
    }

    pub const ALL: [SortOrder; 6] = [
        SortOrder::Newest,
        SortOrder::Oldest,
        SortOrder::NameAsc,
        SortOrder::NameDesc,
        SortOrder::Largest,
        SortOrder::Smallest,
    ];
}

/// One file in `SAVE_DIR`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaItem {
    /// File name, unique within the directory and used as the API path segment.
    pub name: String,
    pub size: u64,
    /// Last modification time as milliseconds since the Unix epoch.
    pub modified_ms: u64,
    pub category: MediaCategory,
    pub mime: String,
}

impl MediaItem {
    /// URL that streams the file inline (range requests supported).
    pub fn url(&self) -> String {
        format!("/api/media/{}", encode_path_segment(&self.name))
    }

    /// URL that serves the file as an attachment.
    pub fn download_url(&self) -> String {
        format!("/api/media/{}?download=1", encode_path_segment(&self.name))
    }
}

/// One page of results plus the counters the pager needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPage {
    pub items: Vec<MediaItem>,
    pub page: u32,
    pub per_page: u32,
    /// Items matching the filter, across all pages.
    pub total: u64,
    pub total_pages: u32,
    /// Files in the directory, ignoring the filter — shown in the header.
    pub total_unfiltered: u64,
    /// Bytes used by all files in the directory, ignoring the filter.
    pub total_bytes: u64,
}

/// Result of a multipart upload: some files may land while others are rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadResponse {
    pub uploaded: Vec<MediaItem>,
    pub failed: Vec<UploadFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadFailure {
    pub name: String,
    pub error: String,
}

/// Error body returned by every failing endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

/// Percent-encodes the characters that would otherwise break out of a single
/// path segment. File names come from Telegram or from uploads, so they can
/// legitimately contain spaces, `#` or `?`.
pub fn encode_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => encoded.push(*byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Human readable byte count, e.g. `1.4 MB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} {}", UNITS[0]) } else { format!("{value:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension() {
        assert_eq!(MediaCategory::from_file_name("photo_ab.JPG"), MediaCategory::Image);
        assert_eq!(MediaCategory::from_file_name("video_ab.mp4"), MediaCategory::Video);
        assert_eq!(MediaCategory::from_file_name("voice_ab.oga"), MediaCategory::Audio);
        assert_eq!(MediaCategory::from_file_name("document_ab.pdf"), MediaCategory::Other);
        assert_eq!(MediaCategory::from_file_name("noextension"), MediaCategory::Other);
    }

    #[test]
    fn encodes_awkward_names() {
        assert_eq!(encode_path_segment("a b#c?.mp4"), "a%20b%23c%3F.mp4");
        assert_eq!(encode_path_segment("photo_1.jpg"), "photo_1.jpg");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
    }
}
