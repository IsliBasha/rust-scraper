use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use rust_embed::RustEmbed;
use scraper_metrics::MetricsHub;
use std::net::SocketAddr;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::routes::{router, AppState};

// Path is relative to this crate's Cargo.toml: crates/scraper-dashboard/ -> ../../dashboard/
#[derive(RustEmbed)]
#[folder = "../../dashboard"]
struct Assets;

async fn static_handler(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let body = axum::body::Body::from(content.data.into_owned());
            ([(header::CONTENT_TYPE, mime.as_ref().to_string())], body).into_response()
        }
        None => (StatusCode::NOT_FOUND, "404 not found").into_response(),
    }
}

pub async fn serve(metrics: MetricsHub, addr: SocketAddr) -> anyhow::Result<()> {
    let state = AppState {
        control: metrics.control_handle(),
        metrics,
    };

    let app = router(state)
        .route("/", get(static_handler))
        .fallback(static_handler)
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive());

    info!(addr = %addr, "dashboard listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
