//! Serves the React SPA from `web/dist`, embedded into the binary in release
//! builds (read from disk in debug builds).

use axum::http::{StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Assets;

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if !path.is_empty()
        && let Some(file) = Assets::get(path)
    {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        // Vite fingerprints everything under assets/, so it can be cached forever.
        let cache = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return (
            [(header::CONTENT_TYPE, mime.as_ref()), (header::CACHE_CONTROL, cache)],
            file.data,
        )
            .into_response();
    }
    // Unknown paths are client-side routes; serve the SPA shell.
    match Assets::get("index.html") {
        Some(index) => Html(index.data).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "web UI not built: run `npm ci && npm run build` in web/",
        )
            .into_response(),
    }
}
