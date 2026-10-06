//! telegram-loader: a Telegram bot that saves incoming media to a folder, and a
//! web server that lists, streams and accepts uploads for that same folder.

mod api;
mod battery;
mod bot;
mod charger;
mod config;
mod media;
mod plug;
mod weburl;

use std::sync::Arc;

use config::Config;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let mut config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("configuration error: {err}");
            std::process::exit(1);
        }
    };

    // Resolve once, so the traversal check in `media::resolve` compares against
    // an absolute path and the log line is unambiguous.
    config.save_dir = match api::ensure_save_dir(&config.save_dir).await {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to prepare the save directory {}: {err}", config.save_dir.display());
            std::process::exit(1);
        }
    };

    let config = Arc::new(config);
    log::info!("Starting telegram-loader, media directory: {}", config.save_dir.display());

    if !config.frontend_dir.is_dir() {
        log::warn!(
            "frontend directory {} does not exist — serving the API only. Build the UI with `trunk build --release` in crates/frontend.",
            config.frontend_dir.display()
        );
    }

    let listener = match TcpListener::bind(config.bind).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("failed to bind {}: {err}", config.bind);
            std::process::exit(1);
        }
    };
    log::info!("Web UI on http://{} — reachable at {}", config.bind, weburl::web_url(&config));

    // Runs alongside the server for the life of the process, like the bot.
    tokio::spawn(charger::run());

    let server = axum::serve(listener, api::router(Arc::clone(&config))).with_graceful_shutdown(shutdown_signal());

    match config.bot_token.clone() {
        Some(token) => {
            // The bot long-polls forever; the server owns the shutdown signal,
            // so the process exits when the server does.
            let bot_config = Arc::clone(&config);
            tokio::spawn(async move { bot::run(bot_config, token).await });

            if let Err(err) = server.await {
                log::error!("web server stopped: {err}");
            }
        }
        None => {
            log::warn!("no TELOXIDE_TOKEN/BOT_TOKEN set — the Telegram bot is disabled, running the web UI only");
            if let Err(err) = server.await {
                log::error!("web server stopped: {err}");
            }
        }
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    log::info!("shutting down");
}
