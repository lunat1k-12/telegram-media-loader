//! Poster frames for videos, made with ffmpeg and cached in `THUMB_DIR`.
//!
//! The cache lives outside `SAVE_DIR` on purpose: nothing in it is media, so it
//! must never show up in the listing or count towards the directory's size.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use tokio::fs;
use tokio::process::Command;
use tokio::sync::Mutex;

/// Longest edge the gallery needs: a card is ~240 px wide, doubled for HiDPI.
/// Smaller videos are left at their own size rather than scaled up.
const SCALE_FILTER: &str = "scale='min(480,iw)':-2";
/// A single frame takes well under a second; anything near this is a hang.
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(30);
/// Where to grab the frame, in seconds. Frame zero is usually black, so one
/// second in is tried first; clips shorter than that fall back to the start.
const SEEK_POSITIONS: [&str; 2] = ["1", "0"];

pub struct Thumbnailer {
    dir: PathBuf,
    ffmpeg: String,
    /// Serialises ffmpeg runs: a page of new videos asks for two dozen
    /// thumbnails at once, and the second request for a name must find the
    /// first one's result instead of repeating the work. The map remembers
    /// videos no frame could be read from, keyed by thumbnail path and holding
    /// the video's mtime, so a broken file is not retried on every page view.
    state: Mutex<HashMap<PathBuf, Option<SystemTime>>>,
    /// Makes temporary file names unique within the process.
    counter: AtomicU64,
}

impl Thumbnailer {
    /// `dir` must already exist.
    pub fn new(dir: PathBuf, ffmpeg: String) -> Self {
        Thumbnailer { dir, ffmpeg, state: Mutex::new(HashMap::new()), counter: AtomicU64::new(0) }
    }

    /// Whether the configured ffmpeg can be started at all.
    pub async fn available(&self) -> bool {
        let mut command = Command::new(&self.ffmpeg);
        command.arg("-version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        command.status().await.is_ok_and(|status| status.success())
    }

    /// `clip.mp4` → `<THUMB_DIR>/clip.mp4.jpg`. The full name is kept so
    /// `clip.mp4` and `clip.mov` do not share a thumbnail.
    fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.jpg"))
    }

    /// Returns the thumbnail for `video`, generating it first when it is
    /// missing or older than the video. `name` is the video's validated file
    /// name. `None` means there is no thumbnail to show — ffmpeg is missing or
    /// could not read a frame — which callers treat as "no preview", not as an
    /// error.
    pub async fn ensure(&self, name: &str, video: &Path) -> Option<PathBuf> {
        let thumb = self.path_for(name);
        if is_fresh(&thumb, video).await {
            return Some(thumb);
        }

        let mut failed = self.state.lock().await;
        // Someone else may have produced it while this call waited.
        if is_fresh(&thumb, video).await {
            return Some(thumb);
        }
        let video_modified = fs::metadata(video).await.ok()?.modified().ok();
        if failed.get(&thumb) == Some(&video_modified) {
            return None;
        }

        // Written under a temporary name and renamed, so a request never sees
        // a half-written image. The name is generated rather than derived from
        // the video's, which could contain a `%d` that ffmpeg would expand.
        let sequence = self.counter.fetch_add(1, Ordering::Relaxed);
        let temp = self.dir.join(format!(".tmp-{}-{sequence}.jpg", std::process::id()));

        for seek in SEEK_POSITIONS {
            match self.grab_frame(video, seek, &temp).await {
                Ok(true) => {
                    if let Err(err) = fs::rename(&temp, &thumb).await {
                        log::warn!("could not store the thumbnail for {name}: {err}");
                        let _ = fs::remove_file(&temp).await;
                        return None;
                    }
                    failed.remove(&thumb);
                    log::debug!("generated a thumbnail for {name}");
                    return Some(thumb);
                }
                Ok(false) => {}
                // ffmpeg itself did not start. Startup already warned about
                // that, and it is not the video's fault, so nothing is cached.
                Err(err) => {
                    log::debug!("could not run {}: {err}", self.ffmpeg);
                    return None;
                }
            }
        }

        let _ = fs::remove_file(&temp).await;
        log::warn!("ffmpeg could not read a frame from {name}, it gets no thumbnail");
        failed.insert(thumb, video_modified);
        None
    }

    /// Drops the thumbnail of a deleted video. A missing one is not an error.
    pub async fn remove(&self, name: &str) {
        let thumb = self.path_for(name);
        let _ = fs::remove_file(&thumb).await;
        self.state.lock().await.remove(&thumb);
    }

    /// One ffmpeg run. `Ok(false)` means it ran but wrote no usable image;
    /// `Err` means it could not be started.
    async fn grab_frame(&self, video: &Path, seek: &str, output: &Path) -> std::io::Result<bool> {
        let mut command = Command::new(&self.ffmpeg);
        command
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
            // Before `-i`, so ffmpeg seeks in the container instead of
            // decoding everything up to that point.
            .args(["-ss", seek, "-i"])
            .arg(video)
            .args(["-map", "0:v:0", "-frames:v", "1", "-vf", SCALE_FILTER, "-q:v", "4"])
            .arg(output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            // A timed-out run is dropped below; this is what reaps it.
            .kill_on_drop(true);

        let output_result = match tokio::time::timeout(FFMPEG_TIMEOUT, command.output()).await {
            Ok(result) => result?,
            Err(_) => {
                log::warn!("ffmpeg timed out on {}", video.display());
                return Ok(false);
            }
        };

        if !output_result.status.success() {
            log::debug!("ffmpeg failed: {}", String::from_utf8_lossy(&output_result.stderr).trim());
            return Ok(false);
        }
        // Seeking past the end of a short clip exits cleanly with no image.
        Ok(fs::metadata(output).await.is_ok_and(|metadata| metadata.len() > 0))
    }
}

/// A thumbnail is reused only while it is at least as new as its video: a
/// deleted file's name can be taken by a different upload.
async fn is_fresh(thumb: &Path, video: &Path) -> bool {
    let (Ok(thumb), Ok(video)) = (fs::metadata(thumb).await, fs::metadata(video).await) else {
        return false;
    };
    match (thumb.modified(), video.modified()) {
        (Ok(thumb), Ok(video)) => thumb >= video,
        // No mtimes on this platform: an existing thumbnail is the best there is.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a real ffmpeg, so it passes vacuously on a machine without one.
    #[tokio::test]
    async fn generates_reuses_and_removes_a_thumbnail() {
        let root = std::env::temp_dir().join(format!("telegram-loader-thumbs-{}", std::process::id()));
        let media = root.join("media");
        let thumbs = root.join("thumbs");
        fs::create_dir_all(&media).await.unwrap();
        fs::create_dir_all(&thumbs).await.unwrap();

        let thumbnailer = Thumbnailer::new(thumbs.clone(), "ffmpeg".to_owned());
        if !thumbnailer.available().await {
            eprintln!("ffmpeg not found, skipping");
            fs::remove_dir_all(&root).await.unwrap();
            return;
        }

        // Half a second long, so the one-second seek misses and the fallback
        // to the first frame is exercised too.
        let video = media.join("clip 100%d.mp4");
        let made = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=duration=0.5:size=640x360:rate=10"])
            .arg(&video)
            .status()
            .await
            .unwrap();
        assert!(made.success());

        let thumb = thumbnailer.ensure("clip 100%d.mp4", &video).await.expect("a thumbnail");
        assert_eq!(thumb, thumbs.join("clip 100%d.mp4.jpg"));
        assert!(fs::metadata(&thumb).await.unwrap().len() > 0);
        // Nothing of it lands next to the media.
        assert!(!fs::try_exists(media.join("clip 100%d.mp4.jpg")).await.unwrap());

        // A second call is served from the cache, not regenerated.
        let modified = fs::metadata(&thumb).await.unwrap().modified().unwrap();
        assert_eq!(thumbnailer.ensure("clip 100%d.mp4", &video).await, Some(thumb.clone()));
        assert_eq!(fs::metadata(&thumb).await.unwrap().modified().unwrap(), modified);

        // Something that is not a video yields no thumbnail and no leftovers.
        let broken = media.join("broken.mp4");
        fs::write(&broken, b"not a video").await.unwrap();
        assert_eq!(thumbnailer.ensure("broken.mp4", &broken).await, None);

        thumbnailer.remove("clip 100%d.mp4").await;
        let mut left = fs::read_dir(&thumbs).await.unwrap();
        assert!(left.next_entry().await.unwrap().is_none());

        fs::remove_dir_all(&root).await.unwrap();
    }
}
