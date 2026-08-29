//! Drop zone and file picker; both funnel into the same multipart POST.

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{DragEvent, FileList, FormData};

use crate::api;
use crate::state::{AppState, UploadProgress};

#[component]
pub fn Uploader() -> impl IntoView {
    let state = AppState::get();
    let input_ref = NodeRef::<html::Input>::new();
    let dragging = RwSignal::new(false);

    let send = move |files: FileList| {
        if files.length() == 0 || state.uploading.get_untracked() {
            return;
        }

        let Ok(form) = FormData::new() else {
            state.upload_note.set(Some(Err("this browser cannot build the upload form".into())));
            return;
        };

        let count = files.length();
        let mut bytes = 0.0;
        for index in 0..count {
            if let Some(file) = files.get(index) {
                bytes += file.size();
                // The field name is ignored by the backend, which keys off the
                // per-part file name instead.
                let _ = form.append_with_blob_and_filename("files", &file, &file.name());
            }
        }

        state.uploading.set(true);
        state.upload_note.set(None);
        // Seeded from the file sizes so the bar starts at 0% rather than
        // waiting for the browser's first progress event. The multipart
        // envelope makes the real total a little larger; the first event
        // corrects it.
        state.upload_progress.set(Some(UploadProgress { sent: 0, total: bytes as u64 }));

        spawn_local(async move {
            let result = api::upload(form, move |sent, total| {
                state.upload_progress.set(Some(UploadProgress { sent, total }));
            })
            .await;
            state.uploading.set(false);
            state.upload_progress.set(None);

            match result {
                Ok(response) => {
                    let uploaded = response.uploaded.len();
                    let note = if response.failed.is_empty() {
                        Ok(format!("Uploaded {uploaded} of {count} file(s)."))
                    } else {
                        let details = response
                            .failed
                            .iter()
                            .map(|failure| format!("{}: {}", failure.name, failure.error))
                            .collect::<Vec<_>>()
                            .join("; ");
                        Err(format!("Uploaded {uploaded} of {count}. Failed — {details}"))
                    };
                    state.upload_note.set(Some(note));
                    if uploaded > 0 {
                        // Show the new files straight away; `Newest first` puts
                        // them on page 1.
                        state.query.update(|query| query.page = 1);
                        state.refresh();
                    }
                }
                Err(err) => state.upload_note.set(Some(Err(err))),
            }
        });
    };

    let pick_files = move |_| {
        if let Some(input) = input_ref.get() {
            input.click();
        }
    };

    view! {
        <div
            class="dropzone"
            class:over=move || dragging.get()
            on:dragover=move |ev: DragEvent| {
                ev.prevent_default();
                dragging.set(true);
            }
            on:dragleave=move |_| dragging.set(false)
            on:drop=move |ev: DragEvent| {
                ev.prevent_default();
                dragging.set(false);
                if let Some(files) = ev.data_transfer().and_then(|transfer| transfer.files()) {
                    send(files);
                }
            }
        >
            <input
                class="hidden-input"
                type="file"
                multiple
                node_ref=input_ref
                on:change=move |_| {
                    if let Some(input) = input_ref.get() {
                        if let Some(files) = input.files() {
                            send(files);
                        }
                        // Clear it, so picking the same file twice fires again.
                        input.set_value("");
                    }
                }
            />

            <Show
                when=move || state.uploading.get()
                fallback=move || {
                    view! {
                        <div>
                            <strong>"Drop files here"</strong>
                            " to add them to the folder, or "
                            <button class="btn primary" on:click=pick_files>
                                "Choose files"
                            </button>
                        </div>
                    }
                }
            >
                <div class="upload-progress">
                    <div
                        class="progress-track"
                        class:indeterminate=move || {
                            state.upload_progress.get().is_none_or(|progress| progress.finishing())
                        }
                    >
                        <div
                            class="progress-fill"
                            style=move || {
                                let percent = state
                                    .upload_progress
                                    .get()
                                    .map(|progress| progress.percent())
                                    .unwrap_or(0);
                                format!("width: {percent}%")
                            }
                        ></div>
                    </div>
                    <div class="progress-label">
                        {move || match state.upload_progress.get() {
                            // The body is out; the server is still writing the
                            // files, which for large uploads is not instant.
                            Some(progress) if progress.finishing() => "Saving files…".to_string(),
                            Some(progress) => {
                                format!(
                                    "Uploading… {}% — {} of {}",
                                    progress.percent(),
                                    shared::format_size(progress.sent),
                                    shared::format_size(progress.total),
                                )
                            }
                            None => "Uploading…".to_string(),
                        }}
                    </div>
                </div>
            </Show>

            {move || {
                state
                    .upload_note
                    .get()
                    .map(|note| match note {
                        Ok(message) => view! { <div class="upload-note ok">{message}</div> },
                        Err(message) => view! { <div class="upload-note err">{message}</div> },
                    })
            }}
        </div>
    }
}
