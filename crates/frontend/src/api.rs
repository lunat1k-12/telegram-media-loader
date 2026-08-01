//! Thin wrappers over the backend's JSON endpoints.

use gloo_net::http::Request;
use shared::{ApiError, MediaPage, UploadResponse};

use crate::state::Query;

/// Fetches one page of the listing.
pub async fn fetch_page(query: &Query) -> Result<MediaPage, String> {
    let url = format!("/api/media?{}", query.to_search());
    let response = Request::get(&url).send().await.map_err(|err| format!("request failed: {err}"))?;

    if !response.ok() {
        return Err(error_message(response).await);
    }
    response.json::<MediaPage>().await.map_err(|err| format!("unexpected response: {err}"))
}

/// Posts a multipart form built from the files the user picked or dropped.
pub async fn upload(form: web_sys::FormData) -> Result<UploadResponse, String> {
    let response = Request::post("/api/media")
        // No explicit Content-Type: the browser has to add the multipart
        // boundary itself.
        .body(form)
        .map_err(|err| format!("could not build the upload: {err}"))?
        .send()
        .await
        .map_err(|err| format!("upload failed: {err}"))?;

    if !response.ok() {
        return Err(error_message(response).await);
    }
    response.json::<UploadResponse>().await.map_err(|err| format!("unexpected response: {err}"))
}

/// Prefers the API's own `{"error": …}` body over a bare status code.
async fn error_message(response: gloo_net::http::Response) -> String {
    let status = response.status();
    match response.json::<ApiError>().await {
        Ok(body) => body.error,
        Err(_) => format!("server returned {status}"),
    }
}
