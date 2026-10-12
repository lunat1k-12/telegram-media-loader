//! HTTP surface: the JSON media API, file streaming/downloads, uploads, and the
//! compiled Leptos frontend served as static files.

use std::path::PathBuf;
use std::sync::{Arc, PoisonError};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path as PathParam, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use shared::{
    ApiError, DEFAULT_PER_PAGE, MAX_PER_PAGE, MediaCategory, MediaPage, PowerStatus, SortOrder, UploadFailure,
    UploadResponse, encode_path_segment,
};
use tokio::fs;
use tokio::io::{AsyncWriteExt, BufWriter};
use tower::ServiceBuilder;
use tower::util::ServiceExt;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::charger::StatusHandle;
use crate::config::Config;
use crate::media::{self, ListRequest, MediaError};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    /// Written by the charge controller, read by `/api/power`.
    pub power: StatusHandle,
}

pub fn router(config: Arc<Config>, power: StatusHandle) -> Router {
    let state = AppState { config: Arc::clone(&config), power };

    let api = Router::new()
        .route("/media", get(list_media).post(upload_media))
        .route("/media/{name}", get(get_media).delete(delete_media))
        .route("/power", get(power_status))
        .route("/health", get(health))
        // Uploads are streamed straight to disk, so the in-memory body limit
        // would only get in the way; `max_upload_bytes` caps them instead.
        .layer(DefaultBodyLimit::disable())
        .with_state(state.clone());

    Router::new()
        .nest("/api", api)
        // Compression is deliberately limited to the frontend bundle: the wasm
        // blob shrinks by roughly two thirds, while media is either already
        // compressed or served in ranges, where re-encoding does not belong.
        .fallback_service(
            ServiceBuilder::new().layer(CompressionLayer::new()).service(frontend_service(&config.frontend_dir)),
        )
        // The frontend is served from the same origin, so CORS only matters for
        // a `trunk serve` dev frontend on another port.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

/// Serves the compiled frontend, falling back to `index.html` so client-side
/// routes survive a page reload.
fn frontend_service(frontend_dir: &std::path::Path) -> ServeDir<ServeFile> {
    ServeDir::new(frontend_dir).fallback(ServeFile::new(frontend_dir.join("index.html")))
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Battery charge and plug state as of the charge controller's last check.
async fn power_status(State(state): State<AppState>) -> Json<PowerStatus> {
    Json(*state.power.lock().unwrap_or_else(PoisonError::into_inner))
}

#[derive(Debug, Deserialize)]
struct ListParams {
    page: Option<u32>,
    per_page: Option<u32>,
    /// `image` | `video` | `audio` | `other`; anything else means "all".
    kind: Option<String>,
    sort: Option<String>,
    /// Substring match on the file name.
    q: Option<String>,
}

async fn list_media(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<MediaPage>, ApiFailure> {
    let request = ListRequest {
        page: params.page.unwrap_or(1).max(1),
        per_page: params.per_page.unwrap_or(DEFAULT_PER_PAGE).clamp(1, MAX_PER_PAGE),
        category: params.kind.as_deref().and_then(MediaCategory::parse),
        sort: params.sort.as_deref().and_then(SortOrder::parse).unwrap_or_default(),
        search: params.q.map(|q| q.trim().to_owned()).filter(|q| !q.is_empty()),
    };

    let page = media::list(&state.config.save_dir, &request).await?;
    Ok(Json(page))
}

#[derive(Debug, Deserialize)]
struct MediaParams {
    /// `?download=1` switches the disposition from inline to attachment.
    download: Option<String>,
}

/// Streams one file. `ServeFile` does the heavy lifting, which is what gives us
/// range requests — without them a browser cannot seek in a video.
async fn get_media(
    State(state): State<AppState>,
    PathParam(name): PathParam<String>,
    Query(params): Query<MediaParams>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let path = media::resolve(&state.config.save_dir, &name).await?;
    let as_attachment = matches!(params.download.as_deref(), Some("1" | "true" | "yes"));

    let serve = ServeFile::new(&path);
    let response = serve.oneshot(request).await.map_err(|err| MediaError::Io(std::io::Error::other(err)))?;
    let mut response = response.map(Body::new);

    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&content_disposition(&name, as_attachment)) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    // A deleted file's name can be taken by a different upload, so the browser
    // has to revalidate (a cheap 304, `ServeFile` sends `Last-Modified`); keep
    // it private because the directory is not meant to be shared by a proxy.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));

    Ok(response)
}

async fn delete_media(
    State(state): State<AppState>,
    PathParam(name): PathParam<String>,
) -> Result<StatusCode, ApiFailure> {
    media::delete(&state.config.save_dir, &name).await?;
    log::info!("deleted {name}");
    Ok(StatusCode::NO_CONTENT)
}

/// Builds a `Content-Disposition` value that survives non-ASCII file names.
fn content_disposition(name: &str, as_attachment: bool) -> String {
    let kind = if as_attachment { "attachment" } else { "inline" };
    let ascii: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_. ".contains(c) { c } else { '_' }).collect();

    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{}", encode_path_segment(name))
}

/// Accepts a multipart form with one or more file fields.
///
/// A failing file does not abort the rest of the upload; the response lists
/// what landed and what did not.
async fn upload_media(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, ApiFailure> {
    let mut response = UploadResponse::default();

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|err| ApiFailure { status: StatusCode::BAD_REQUEST, message: format!("malformed upload: {err}") })?
    {
        let Some(raw_name) = field.file_name().map(str::to_owned) else {
            // A plain form field, not a file — skip it.
            continue;
        };

        let name = media::sanitize_upload_name(&raw_name);
        let destination = match media::unique_path(&state.config.save_dir, &name).await {
            Ok(path) => path,
            Err(err) => {
                response.failed.push(UploadFailure { name: raw_name, error: err.to_string() });
                continue;
            }
        };

        // Same partial-then-rename dance the bot uses, so a dropped connection
        // never leaves a truncated file in the listing.
        let partial = media::with_partial_suffix(&destination);
        let file = match fs::File::create(&partial).await {
            Ok(file) => file,
            Err(err) => {
                response.failed.push(UploadFailure { name: raw_name, error: err.to_string() });
                continue;
            }
        };

        let mut writer = BufWriter::new(file);
        let mut written = 0u64;
        let mut failure: Option<String> = None;

        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    written += chunk.len() as u64;
                    if written > state.config.max_upload_bytes {
                        failure = Some(format!(
                            "file is larger than the {} limit",
                            shared::format_size(state.config.max_upload_bytes)
                        ));
                        break;
                    }
                    if let Err(err) = writer.write_all(&chunk).await {
                        failure = Some(err.to_string());
                        break;
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    failure = Some(err.to_string());
                    break;
                }
            }
        }

        if failure.is_none()
            && let Err(err) = writer.flush().await
        {
            failure = Some(err.to_string());
        }
        drop(writer);

        if let Some(error) = failure {
            let _ = fs::remove_file(&partial).await;
            log::warn!("upload of {raw_name} failed: {error}");
            response.failed.push(UploadFailure { name: raw_name, error });
            continue;
        }

        if let Err(err) = fs::rename(&partial, &destination).await {
            let _ = fs::remove_file(&partial).await;
            response.failed.push(UploadFailure { name: raw_name, error: err.to_string() });
            continue;
        }

        match media::describe(&destination).await {
            Ok(item) => {
                log::info!("uploaded {} ({} bytes)", item.name, item.size);
                response.uploaded.push(item);
            }
            Err(err) => response.failed.push(UploadFailure { name: raw_name, error: err.to_string() }),
        }
    }

    Ok(Json(response))
}

/// Any failing endpoint answers with the shared `ApiError` body.
pub struct ApiFailure {
    status: StatusCode,
    message: String,
}

impl From<MediaError> for ApiFailure {
    fn from(err: MediaError) -> Self {
        let status = match err {
            MediaError::InvalidName => StatusCode::BAD_REQUEST,
            MediaError::NotFound => StatusCode::NOT_FOUND,
            MediaError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiFailure { status, message: err.to_string() }
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            log::error!("request failed: {}", self.message);
        }
        (self.status, Json(ApiError { error: self.message })).into_response()
    }
}

/// Convenience for `main`, which needs the directory to exist before serving.
pub async fn ensure_save_dir(save_dir: &PathBuf) -> std::io::Result<PathBuf> {
    fs::create_dir_all(save_dir).await?;
    fs::canonicalize(save_dir).await
}
