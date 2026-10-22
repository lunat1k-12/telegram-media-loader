//! Reading, filtering and paginating the contents of `SAVE_DIR`, plus the file
//! name rules that keep requests inside that directory.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rand::seq::SliceRandom;
use shared::{MAX_PER_PAGE, MediaCategory, MediaItem, MediaPage, SortOrder};
use tokio::fs;

/// Suffix used for in-flight downloads; those files are not listed.
pub const PARTIAL_SUFFIX: &str = ".part";
/// Longest file name we will write, in bytes. Comfortably under the 255 that
/// ext4/APFS allow, leaving room for a `_12` de-duplication suffix.
const MAX_NAME_LEN: usize = 200;

#[derive(Debug)]
pub enum MediaError {
    /// The name could never refer to a file in `SAVE_DIR`.
    InvalidName,
    NotFound,
    Io(std::io::Error),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MediaError::InvalidName => write!(f, "invalid file name"),
            MediaError::NotFound => write!(f, "file not found"),
            MediaError::Io(err) => write!(f, "{err}"),
        }
    }
}

impl From<std::io::Error> for MediaError {
    fn from(err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::NotFound { MediaError::NotFound } else { MediaError::Io(err) }
    }
}

/// Query the listing endpoint understands, already validated.
#[derive(Debug, Clone)]
pub struct ListRequest {
    pub page: u32,
    pub per_page: u32,
    pub category: Option<MediaCategory>,
    pub sort: SortOrder,
    /// Case-insensitive substring match on the file name.
    pub search: Option<String>,
}

/// Rejects anything that is not a plain file name directly inside `SAVE_DIR`.
///
/// Path traversal (`../`), absolute paths, nested directories and the hidden
/// files the bot uses internally are all refused here, before any filesystem
/// call happens.
pub fn validate_name(name: &str) -> Result<&str, MediaError> {
    let invalid = name.is_empty()
        || name.len() > MAX_NAME_LEN + 16
        || name.starts_with('.')
        || name.ends_with(PARTIAL_SUFFIX)
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.chars().any(|c| c.is_control());

    if invalid { Err(MediaError::InvalidName) } else { Ok(name) }
}

/// Resolves a request path segment to a file inside `save_dir`.
///
/// `validate_name` already rules out traversal; the canonicalised prefix check
/// is a second line of defence that also catches symlinks pointing outside.
pub async fn resolve(save_dir: &Path, name: &str) -> Result<PathBuf, MediaError> {
    let name = validate_name(name)?;
    let candidate = save_dir.join(name);

    let resolved = fs::canonicalize(&candidate).await?;
    let root = fs::canonicalize(save_dir).await?;
    if !resolved.starts_with(&root) {
        return Err(MediaError::InvalidName);
    }
    if !fs::metadata(&resolved).await?.is_file() {
        return Err(MediaError::NotFound);
    }

    Ok(resolved)
}

/// Removes one file from `save_dir`, under the same name rules as `resolve`.
pub async fn delete(save_dir: &Path, name: &str) -> Result<(), MediaError> {
    let path = resolve(save_dir, name).await?;
    fs::remove_file(&path).await?;
    Ok(())
}

/// Lists the directory, applies the filter and returns the requested page.
///
/// The whole directory is read on every request. That is fine for the tens of
/// thousands of files a bot inbox realistically holds; if it ever stops being
/// fine, this is the function to put an index in front of.
pub async fn list(save_dir: &Path, request: &ListRequest) -> Result<MediaPage, MediaError> {
    let mut all = scan(save_dir).await?;
    let total_bytes = all.iter().map(|item| item.size).sum();

    let total_unfiltered = all.len() as u64;

    let search = request.search.as_ref().map(|term| term.to_lowercase());
    all.retain(|item| {
        let category_matches = request.category.is_none_or(|category| item.category == category);
        let search_matches = search.as_ref().is_none_or(|term| item.name.to_lowercase().contains(term));
        category_matches && search_matches
    });

    match request.sort {
        // Ties are broken by name so paging is stable when many files share a
        // timestamp, which is common for a batch of uploads.
        SortOrder::Newest => all.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then_with(|| a.name.cmp(&b.name))),
        SortOrder::Oldest => all.sort_by(|a, b| a.modified_ms.cmp(&b.modified_ms).then_with(|| a.name.cmp(&b.name))),
        SortOrder::NameAsc => all.sort_by(|a, b| a.name.cmp(&b.name)),
        SortOrder::NameDesc => all.sort_by(|a, b| b.name.cmp(&a.name)),
        SortOrder::Largest => all.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))),
        SortOrder::Smallest => all.sort_by(|a, b| a.size.cmp(&b.size).then_with(|| a.name.cmp(&b.name))),
    }

    let total = all.len() as u64;
    let per_page = request.per_page.clamp(1, MAX_PER_PAGE);
    let total_pages = total.div_ceil(per_page as u64).max(1) as u32;
    let page = request.page.clamp(1, total_pages);

    let start = ((page - 1) as usize) * per_page as usize;
    let items = all.into_iter().skip(start).take(per_page as usize).collect();

    Ok(MediaPage { items, page, per_page, total, total_pages, total_unfiltered, total_bytes })
}

/// Up to `count` distinct files picked at random, optionally of one category.
pub async fn random(
    save_dir: &Path,
    category: Option<MediaCategory>,
    count: usize,
) -> Result<Vec<MediaItem>, MediaError> {
    let mut all = scan(save_dir).await?;
    all.retain(|item| category.is_none_or(|category| item.category == category));

    let mut rng = rand::rng();
    let (picked, _) = all.partial_shuffle(&mut rng, count);
    Ok(picked.to_vec())
}

/// Every listable file in `save_dir`, in directory order.
async fn scan(save_dir: &Path) -> Result<Vec<MediaItem>, MediaError> {
    let mut all = Vec::new();

    let mut entries = fs::read_dir(save_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if validate_name(&name).is_err() {
            continue;
        }

        let metadata = match entry.metadata().await {
            Ok(metadata) if metadata.is_file() => metadata,
            // A file removed between read_dir and stat is simply not listed.
            _ => continue,
        };

        all.push(MediaItem {
            category: MediaCategory::from_file_name(&name),
            mime: mime_guess::from_path(&name).first_or_octet_stream().to_string(),
            size: metadata.len(),
            modified_ms: modified_ms(&metadata),
            name,
        });
    }

    Ok(all)
}

/// Describes one file, for the response to an upload.
pub async fn describe(path: &Path) -> Result<MediaItem, MediaError> {
    let metadata = fs::metadata(path).await?;
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();

    Ok(MediaItem {
        category: MediaCategory::from_file_name(&name),
        mime: mime_guess::from_path(&name).first_or_octet_stream().to_string(),
        size: metadata.len(),
        modified_ms: modified_ms(&metadata),
        name,
    })
}

/// Turns a browser-supplied file name into one that is safe to write.
///
/// Anything that could escape the directory or confuse the listing is replaced
/// with `_`; an unusable name falls back to `upload`.
pub fn sanitize_upload_name(raw: &str) -> String {
    // Browsers normally send a bare name, but a directory upload can include
    // path segments — keep only the last one.
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();

    let mut cleaned: String =
        base.chars().map(|c| if c.is_control() || matches!(c, '/' | '\\' | '\0') { '_' } else { c }).collect();

    while cleaned.starts_with('.') {
        cleaned.remove(0);
    }
    if cleaned.len() > MAX_NAME_LEN {
        // Truncate the stem rather than the extension, which the UI relies on.
        let (stem, extension) = match cleaned.rsplit_once('.') {
            Some((stem, extension)) if extension.len() <= 12 => (stem.to_owned(), format!(".{extension}")),
            _ => (cleaned.clone(), String::new()),
        };
        let keep = MAX_NAME_LEN.saturating_sub(extension.len());
        let mut truncated = stem;
        truncated.truncate(floor_char_boundary(&truncated, keep));
        cleaned = format!("{truncated}{extension}");
    }

    if cleaned.trim().is_empty() || cleaned.ends_with(PARTIAL_SUFFIX) { "upload".to_owned() } else { cleaned }
}

/// Finds a free path for `name`, appending `_1`, `_2`, … on collision.
///
/// Existing files are never overwritten: the bot names by `file_unique_id`, and
/// silently replacing a file a user already has would lose data.
pub async fn unique_path(save_dir: &Path, name: &str) -> Result<PathBuf, MediaError> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_owned(), format!(".{extension}")),
        _ => (name.to_owned(), String::new()),
    };

    let mut candidate = save_dir.join(name);
    let mut counter = 1u32;
    while fs::try_exists(&candidate).await? || fs::try_exists(with_partial_suffix(&candidate)).await? {
        candidate = save_dir.join(format!("{stem}_{counter}{extension}"));
        counter += 1;
        if counter > 10_000 {
            return Err(MediaError::Io(std::io::Error::other("could not find a free file name")));
        }
    }

    Ok(candidate)
}

/// `foo.mp4` → `foo.mp4.part`, the temporary name used while writing.
pub fn with_partial_suffix(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(PARTIAL_SUFFIX);
    path.with_file_name(name)
}

fn modified_ms(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0))
}

/// `str::floor_char_boundary` is still unstable, so truncation does it by hand.
fn floor_char_boundary(value: &str, index: usize) -> usize {
    if index >= value.len() {
        return value.len();
    }
    let mut index = index;
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_hidden_files() {
        assert!(validate_name("../../etc/passwd").is_err());
        assert!(validate_name("sub/dir.jpg").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name(".DS_Store").is_err());
        assert!(validate_name("photo.jpg.part").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("photo_AgAD.jpg").is_ok());
        assert!(validate_name("holiday video (1).mp4").is_ok());
    }

    #[tokio::test]
    async fn deletes_only_files_inside_the_directory() {
        let dir = std::env::temp_dir().join(format!("telegram-loader-delete-{}", std::process::id()));
        let inner = dir.join("media");
        fs::create_dir_all(&inner).await.unwrap();
        fs::write(inner.join("photo.jpg"), b"x").await.unwrap();
        fs::write(dir.join("outside.jpg"), b"x").await.unwrap();

        assert!(matches!(delete(&inner, "../outside.jpg").await, Err(MediaError::InvalidName)));
        assert!(fs::try_exists(dir.join("outside.jpg")).await.unwrap());

        delete(&inner, "photo.jpg").await.unwrap();
        assert!(!fs::try_exists(inner.join("photo.jpg")).await.unwrap());
        assert!(matches!(delete(&inner, "photo.jpg").await, Err(MediaError::NotFound)));

        fs::remove_dir_all(&dir).await.unwrap();
    }

    #[test]
    fn sanitizes_upload_names() {
        assert_eq!(sanitize_upload_name("../../evil.sh"), "evil.sh");
        assert_eq!(sanitize_upload_name("C:\\photos\\a.jpg"), "a.jpg");
        assert_eq!(sanitize_upload_name(".hidden"), "hidden");
        assert_eq!(sanitize_upload_name("   "), "upload");
        assert_eq!(sanitize_upload_name("clip.mp4"), "clip.mp4");
    }

    #[test]
    fn truncates_long_names_but_keeps_the_extension() {
        let long = format!("{}.mp4", "a".repeat(500));
        let sanitized = sanitize_upload_name(&long);
        assert!(sanitized.len() <= MAX_NAME_LEN);
        assert!(sanitized.ends_with(".mp4"));
    }
}
