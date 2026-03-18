use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use std::convert::Infallible;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::app::bookmarks::ProgressEvent;
use crate::web::extractors::AuthUser;
use crate::web::state::{AppState, Bookmarks};

pub async fn fix_images(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Response {
    let user_id = user.id;

    {
        let mut jobs = state.active_image_fix_jobs.lock().unwrap();
        if jobs.contains(&user_id) {
            return StatusCode::CONFLICT.into_response();
        }
        jobs.insert(user_id);
    }

    let (tx, rx) = mpsc::channel::<ProgressEvent>(32);
    let jobs = state.active_image_fix_jobs.clone();
    let screenshot_url = state.config.screenshot_service_url.clone();

    tokio::spawn(async move {
        match &state.bookmarks {
            Bookmarks::Local(svc) => {
                svc.fix_missing_images(user_id, screenshot_url.as_deref(), tx).await
            }
            Bookmarks::S3(svc) => {
                svc.fix_missing_images(user_id, screenshot_url.as_deref(), tx).await
            }
        }
        jobs.lock().unwrap().remove(&user_id);
    });

    let stream = ReceiverStream::new(rx).map(|event| {
        let json = serde_json::to_string(&event).unwrap_or_default();
        Ok::<_, Infallible>(Event::default().data(json))
    });

    Sse::new(stream).into_response()
}

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new().route("/fix-images", axum::routing::post(fix_images))
}
