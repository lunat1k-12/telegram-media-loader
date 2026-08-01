//! Drop zone and file picker; both funnel into the same multipart POST.

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{DragEvent, FileList, FormData};

use crate::api;
use crate::state::AppState;

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
        for index in 0..count {
            if let Some(file) = files.get(index) {
                // The field name is ignored by the backend, which keys off the
                // per-part file name instead.
                let _ = form.append_with_blob_and_filename("files", &file, &file.name());
            }
        }

        state.uploading.set(true);
        state.upload_note.set(None);

        spawn_local(async move {
            let result = api::upload(form).await;
            state.uploading.set(false);

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
                <div>"Uploading…"</div>
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
