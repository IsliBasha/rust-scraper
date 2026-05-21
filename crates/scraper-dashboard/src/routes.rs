use axum::{
    extract::State,
    response::sse::{Event, Sse},
    routing::get,
    Json, Router,
};
use scraper_metrics::{ControlHandle, MetricsHub};
use std::convert::Infallible;

#[derive(Clone)]
pub struct AppState {
    pub metrics: MetricsHub,
    pub control: ControlHandle,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/metrics/stream", get(metrics_stream))
        .route("/api/metrics/snapshot", get(metrics_snapshot))
        .route("/api/control/shutdown", get(control_shutdown))
        .with_state(state)
}

async fn metrics_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    crate::sse::metrics_sse(state.metrics).await
}

async fn metrics_snapshot(State(state): State<AppState>) -> Json<serde_json::Value> {
    let snap = state.metrics.snapshot();
    Json(serde_json::json!({
        "pending":     snap.urls_pending,
        "in_progress": snap.urls_in_progress,
        "done":        snap.urls_done,
        "failed":      snap.urls_failed,
        "bytes":       snap.bytes_downloaded,
        "rps":         snap.requests_per_second,
    }))
}

async fn control_shutdown(State(state): State<AppState>) -> &'static str {
    state.control.shutdown();
    "shutdown signal sent"
}
