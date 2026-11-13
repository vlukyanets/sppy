//! `GET /media/{file_id}` and `GET /media/{file_id}/sub/{index}` with Range support.

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::AppState;

#[derive(Deserialize)]
pub struct TokenQuery {
    t: Option<String>,
}

async fn serve(
    state: &AppState,
    token: Option<String>,
    file_id: &str,
    sub: Option<u32>,
    companion: bool,
    req: Request,
) -> Response {
    if !token.is_some_and(|t| state.media.session_valid(&t)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(path) = state.media.resolve(file_id, sub, companion) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match ServeFile::new(path).oneshot(req).await {
        Ok(res) => res.map(Body::new),
        Err(e) => match e {},
    }
}

pub async fn file(
    State(state): State<AppState>,
    Path(file_id): Path<String>,
    Query(q): Query<TokenQuery>,
    req: Request,
) -> Response {
    serve(&state, q.t, &file_id, None, false, req).await
}

pub async fn subtitle(
    State(state): State<AppState>,
    Path((file_id, index)): Path<(String, u32)>,
    Query(q): Query<TokenQuery>,
    req: Request,
) -> Response {
    serve(&state, q.t, &file_id, Some(index), false, req).await
}

/// The `.sub` data file of a VobSub (`.idx`) subtitle.
pub async fn subtitle_companion(
    State(state): State<AppState>,
    Path((file_id, index)): Path<(String, u32)>,
    Query(q): Query<TokenQuery>,
    req: Request,
) -> Response {
    serve(&state, q.t, &file_id, Some(index), true, req).await
}
