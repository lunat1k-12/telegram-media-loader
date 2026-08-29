//! Thin wrappers over the backend's JSON endpoints.

use gloo_net::http::Request;
use shared::{ApiError, MediaPage, UploadResponse};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::{ProgressEvent, XmlHttpRequest};

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
///
/// `on_progress` is called with `(bytes sent, total bytes)` as the body goes
/// out. That is why this goes through `XMLHttpRequest` rather than `fetch`,
/// which reports nothing until the whole request has been sent.
pub async fn upload(
    form: web_sys::FormData,
    on_progress: impl FnMut(u64, u64) + 'static,
) -> Result<UploadResponse, String> {
    let request = XmlHttpRequest::new().map_err(|_| "this browser cannot upload files".to_string())?;
    request.open("POST", "/api/media").map_err(|err| format!("could not build the upload: {err:?}"))?;

    // Held until the request settles; dropping it detaches the listener.
    let mut on_progress = on_progress;
    let progress = Closure::<dyn FnMut(ProgressEvent)>::new(move |event: ProgressEvent| {
        if event.length_computable() {
            on_progress(event.loaded() as u64, event.total() as u64);
        }
    });
    if let Ok(upload) = request.upload() {
        upload.set_onprogress(Some(progress.as_ref().unchecked_ref()));
    }

    // `loadend` covers success, network failure and abort alike, so one
    // listener is enough and the status tells the three apart afterwards.
    let settled = js_sys::Promise::new(&mut |resolve, _reject| {
        let request = request.clone();
        let done = Closure::once_into_js(move || {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        });
        request.set_onloadend(Some(done.unchecked_ref()));
    });

    request.send_with_opt_form_data(Some(&form)).map_err(|err| format!("upload failed: {err:?}"))?;
    JsFuture::from(settled).await.map_err(|_| "upload failed".to_string())?;
    drop(progress);

    let status = request.status().unwrap_or(0);
    let body = request.response_text().ok().flatten().unwrap_or_default();

    // A zero status means the request never reached the server: offline,
    // refused connection, or a navigation away mid-upload.
    if status == 0 {
        return Err("upload failed: the connection dropped".into());
    }
    if !(200..300).contains(&status) {
        return Err(match serde_json::from_str::<ApiError>(&body) {
            Ok(error) => error.error,
            Err(_) => format!("server returned {status}"),
        });
    }
    serde_json::from_str::<UploadResponse>(&body).map_err(|err| format!("unexpected response: {err}"))
}

/// Prefers the API's own `{"error": …}` body over a bare status code.
async fn error_message(response: gloo_net::http::Response) -> String {
    let status = response.status();
    match response.json::<ApiError>().await {
        Ok(body) => body.error,
        Err(_) => format!("server returned {status}"),
    }
}
