//! The Telegram side: every incoming photo/video/voice is written to
//! `SAVE_DIR`, the same directory the web UI serves.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use teloxide::prelude::*;
use teloxide::types::{FileId, FileUniqueId, MediaKind, MessageKind};
use teloxide::utils::command::BotCommands;
use tokio::fs;
use tokio::io::{AsyncWriteExt, BufWriter};

use crate::config::Config;
use crate::{media, weburl};

type SaveResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Commands the bot answers. The descriptions are what Telegram shows in the
/// command menu, so they are written for a user, not for the log.
#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase", description = "Commands:")]
enum Command {
    #[command(description = "link to the media library web page")]
    Webpage,
    #[command(description = "what this bot does")]
    Help,
    #[command(description = "same as /help")]
    Start,
}

/// Client for file downloads only, cloned into every handler (it is an `Arc`
/// inside).
///
/// The bot's own client has a 17 s total deadline per request, which also
/// covers reading the body — a large video on a slow link hits it mid-transfer
/// and fails with "error decoding response body". This one has no total
/// deadline and instead gives up only when the connection stalls.
#[derive(Clone)]
struct DownloadClient(reqwest::Client);

impl DownloadClient {
    fn new() -> reqwest::Result<Self> {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(60))
            .tcp_nodelay(true)
            .build()
            .map(Self)
    }
}

/// Long-polls Telegram until the process is stopped.
pub async fn run(config: Arc<Config>, token: String) {
    let bot = Bot::new(token);
    let downloads = match DownloadClient::new() {
        Ok(client) => client,
        Err(err) => {
            log::error!("could not create the download client, the bot is disabled: {err}");
            return;
        }
    };

    // Populates the "/" menu in the Telegram client. A failure here is not
    // fatal — the commands still work, they are just not suggested.
    if let Err(err) = bot.set_my_commands(Command::bot_commands()).await {
        log::warn!("could not register the command list: {err}");
    }

    let handler = Update::filter_message()
        .branch(dptree::entry().filter_command::<Command>().endpoint(on_command))
        // Anything that is not a known command is treated as possible media.
        .branch(dptree::endpoint(on_message));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![config, downloads])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

async fn on_command(bot: Bot, message: Message, command: Command, config: Arc<Config>) -> ResponseResult<()> {
    if let Command::Webpage = command {
        log::info!("/webpage in chat {} → {}", message.chat.id, weburl::web_url(&config));
    }

    bot.send_message(message.chat.id, reply_to(command, &config)).await?;
    Ok(())
}

/// The text each command answers with.
fn reply_to(command: Command, config: &Config) -> String {
    match command {
        Command::Webpage => {
            format!("Media library: {}\n\nReachable from this network only.", weburl::web_url(config))
        }
        Command::Help | Command::Start => {
            format!("Send me a photo, video or voice message and I will save it.\n\n{}", Command::descriptions())
        }
    }
}

async fn on_message(
    bot: Bot,
    message: Message,
    config: Arc<Config>,
    downloads: DownloadClient,
) -> ResponseResult<()> {
    let MessageKind::Common(common) = &message.kind else {
        log::debug!("skipping non-common message");
        return Ok(());
    };

    let Some((kind, file_id, unique_id)) = media_file(&common.media_kind) else {
        log::debug!("skipping unsupported media kind");
        return Ok(());
    };

    let reply = match save_file(&bot, &downloads, &config.save_dir, kind, &file_id, &unique_id).await {
        Ok(path) => {
            log::info!("saved {kind} to {}", path.display());
            format!("{kind} saved as {}", file_name(&path))
        }
        Err(err) => {
            log::error!("failed to save {kind}: {err}");
            format!("failed to save {kind}: {err}")
        }
    };

    if let Err(err) = bot.send_message(message.chat.id, reply).await {
        log::error!("failed to send the reply: {err}");
    }

    Ok(())
}

/// Picks the downloadable file out of a message, if the message carries one.
fn media_file(media: &MediaKind) -> Option<(&'static str, FileId, FileUniqueId)> {
    let (kind, meta) = match media {
        MediaKind::Photo(m) => {
            // Telegram sends the same photo in several sizes; keep only the largest one.
            let largest = m.photo.iter().max_by_key(|size| u64::from(size.width) * u64::from(size.height))?;
            ("photo", &largest.file)
        }
        MediaKind::Video(m) => ("video", &m.video.file),
        MediaKind::VideoNote(m) => ("video_note", &m.video_note.file),
        MediaKind::Animation(m) => ("animation", &m.animation.file),
        MediaKind::Voice(m) => ("voice", &m.voice.file),
        MediaKind::Audio(m) => ("audio", &m.audio.file),
        // Photos and videos sent "as a file" arrive as documents.
        MediaKind::Document(m) => ("document", &m.document.file),
        _ => return None,
    };

    Some((kind, meta.id.clone(), meta.unique_id.clone()))
}

async fn save_file(
    bot: &Bot,
    downloads: &DownloadClient,
    save_dir: &Path,
    kind: &str,
    file_id: &FileId,
    unique_id: &FileUniqueId,
) -> SaveResult<PathBuf> {
    let file = bot.get_file(file_id.clone()).await?;

    // Telegram's own path carries the real extension (e.g. `voice/file_3.oga`).
    let extension = Path::new(&file.path).extension().and_then(|ext| ext.to_str()).unwrap_or("bin");
    let destination = save_dir.join(format!("{kind}_{unique_id}.{extension}"));

    // `file_unique_id` is stable, so a file sent twice is already on disk.
    if fs::try_exists(&destination).await? {
        log::info!("{} already downloaded", destination.display());
        return Ok(destination);
    }

    // Download into a temporary name so an interrupted transfer never leaves a truncated file.
    let partial = media::with_partial_suffix(&destination);
    let mut writer = BufWriter::new(fs::File::create(&partial).await?);
    teloxide::net::download_file(&downloads.0, bot.api_url(), bot.token(), &file.path, &mut writer).await?;
    writer.flush().await?;
    fs::rename(&partial, &destination).await?;

    Ok(destination)
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_webpage_command() {
        assert!(matches!(Command::parse("/webpage", "loaderbot"), Ok(Command::Webpage)));
        // Group chats append the bot's username to the command.
        assert!(matches!(Command::parse("/webpage@loaderbot", "loaderbot"), Ok(Command::Webpage)));
        assert!(matches!(Command::parse("/help", "loaderbot"), Ok(Command::Help)));
        // Plain text falls through to the media branch instead.
        assert!(Command::parse("webpage", "loaderbot").is_err());
        assert!(Command::parse("/unknown", "loaderbot").is_err());
    }

    /// A config that does not depend on the environment the tests run in.
    fn test_config(web_url: Option<&str>) -> Config {
        Config {
            save_dir: PathBuf::from("downloads"),
            bind: "0.0.0.0:8080".parse().unwrap(),
            frontend_dir: PathBuf::from("dist/web"),
            max_upload_bytes: 1,
            bot_token: None,
            web_url: web_url.map(str::to_owned),
        }
    }

    #[test]
    fn webpage_answers_with_the_configured_url() {
        let config = test_config(Some("http://192.168.1.73:8080"));

        let reply = reply_to(Command::Webpage, &config);
        assert!(reply.contains("http://192.168.1.73:8080"), "{reply}");
    }

    #[test]
    fn help_lists_every_command() {
        let reply = reply_to(Command::Help, &test_config(None));

        assert!(reply.contains("/webpage"), "{reply}");
        assert!(reply.contains("/help"), "{reply}");
    }

    #[test]
    fn lists_webpage_in_the_command_menu() {
        let commands = Command::bot_commands();
        assert!(commands.iter().any(|command| command.command == "/webpage"), "{commands:?}");
    }
}
