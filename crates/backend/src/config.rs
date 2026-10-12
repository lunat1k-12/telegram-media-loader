//! Runtime configuration, all of it from the environment.

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Directory every incoming photo/video/voice is written to, and the directory
/// the web UI lists.
pub const DEFAULT_SAVE_DIR: &str = "downloads";
/// Where video thumbnails are cached. Kept out of the save directory so they
/// are never listed as media.
const DEFAULT_THUMB_DIR: &str = "thumbnails";
const DEFAULT_FFMPEG_BIN: &str = "ffmpeg";
const DEFAULT_BIND: &str = "0.0.0.0:8080";
/// Where `trunk build` puts the compiled frontend.
const DEFAULT_FRONTEND_DIR: &str = "dist/web";
/// Refuse a single uploaded file larger than this.
const DEFAULT_MAX_UPLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Config {
    pub save_dir: PathBuf,
    /// Cache of generated video thumbnails; must not be inside `save_dir`.
    pub thumb_dir: PathBuf,
    /// Name or path of the ffmpeg executable that makes them.
    pub ffmpeg_bin: String,
    pub bind: SocketAddr,
    /// Directory holding `index.html` and the wasm bundle. When it is missing
    /// the server still runs and only serves the API.
    pub frontend_dir: PathBuf,
    pub max_upload_bytes: u64,
    /// `None` disables the bot and runs the web server alone, which is handy
    /// for browsing an existing folder locally.
    pub bot_token: Option<String>,
    /// Overrides the address `/webpage` hands out. Needed whenever the server
    /// cannot see the address clients use — behind a reverse proxy, or in a
    /// container, where the detected address is the container's own.
    pub web_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let save_dir = PathBuf::from(env_or(&["SAVE_DIR"], DEFAULT_SAVE_DIR));
        let thumb_dir = PathBuf::from(env_or(&["THUMB_DIR"], DEFAULT_THUMB_DIR));
        let ffmpeg_bin = env_or(&["FFMPEG_BIN"], DEFAULT_FFMPEG_BIN);
        let bind_raw = env_or(&["BIND_ADDR"], DEFAULT_BIND);
        let bind: SocketAddr = bind_raw.parse().map_err(|err| format!("invalid BIND_ADDR `{bind_raw}`: {err}"))?;

        let frontend_dir = PathBuf::from(env_or(&["FRONTEND_DIR"], DEFAULT_FRONTEND_DIR));

        let max_upload_bytes = match env::var("MAX_UPLOAD_BYTES") {
            Ok(raw) => raw.parse().map_err(|err| format!("invalid MAX_UPLOAD_BYTES `{raw}`: {err}"))?,
            Err(_) => DEFAULT_MAX_UPLOAD_BYTES,
        };

        // TELOXIDE_TOKEN is teloxide's own convention; BOT_TOKEN is accepted as
        // the more obvious name.
        let bot_token =
            env::var("TELOXIDE_TOKEN").or_else(|_| env::var("BOT_TOKEN")).ok().filter(|t| !t.trim().is_empty());

        // Trailing slashes would double up when the URL is printed, so drop them.
        let web_url = env::var("WEB_URL")
            .or_else(|_| env::var("PUBLIC_URL"))
            .ok()
            .map(|url| url.trim().trim_end_matches('/').to_owned())
            .filter(|url| !url.is_empty());

        Ok(Config { save_dir, thumb_dir, ffmpeg_bin, bind, frontend_dir, max_upload_bytes, bot_token, web_url })
    }
}

fn env_or(names: &[&str], fallback: &str) -> String {
    for name in names {
        if let Ok(value) = env::var(name)
            && !value.trim().is_empty()
        {
            return value;
        }
    }
    fallback.to_owned()
}
