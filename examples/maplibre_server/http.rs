use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::response::Response;
use axum::routing;

use crate::state::AppState;

const HTML: &[u8] = include_bytes!("../maplibre/index.html");
const MAPLIBRE_JS: &[u8] = include_bytes!("../maplibre/maplibre-gl.js");
const MAPLIBRE_CSS: &[u8] = include_bytes!("../maplibre/styles/maplibre-gl.css");
const MAPLIBRE_LICENSE: &[u8] = include_bytes!("../maplibre/LICENSE-maplibre-gl.txt");

pub(crate) fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .fallback(routing::any(dispatch))
        .with_state(state)
}

async fn dispatch(
    State(state): State<Arc<AppState>>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    let path = uri.path();
    if method == Method::POST && path == "/sources" {
        let result = match std::str::from_utf8(&body) {
            Ok(body) => state.configure(body).await,
            Err(_) => Err("request body must be UTF-8".to_owned()),
        };
        return match result {
            Ok(()) => text_response(StatusCode::OK, "sources configured\n"),
            Err(error) => text_response(StatusCode::BAD_REQUEST, &format!("{error}\n")),
        };
    }
    if method != Method::GET {
        return text_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n");
    }

    match path {
        "/" => response(StatusCode::OK, HTML, "text/html; charset=utf-8"),
        "/maplibre-gl.js" => response(
            StatusCode::OK,
            MAPLIBRE_JS,
            "application/javascript; charset=utf-8",
        ),
        "/maplibre-gl.css" => response(StatusCode::OK, MAPLIBRE_CSS, "text/css; charset=utf-8"),
        "/maplibre-gl-license.txt" => response(
            StatusCode::OK,
            MAPLIBRE_LICENSE,
            "text/plain; charset=utf-8",
        ),
        "/health" => text_response(StatusCode::OK, "ok\n"),
        _ => {
            if let Some(bytes) = crate::state::fixture(path) {
                return response(StatusCode::OK, bytes, "application/vnd.mapbox-vector-tile");
            }
            let Some((z, x, y)) = tile_coordinates(path) else {
                return text_response(StatusCode::NOT_FOUND, "not found\n");
            };
            match state.compose_tile(z, x, y).await {
                Ok(tile) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/vnd.mapbox-vector-tile")
                    .header("cache-control", "no-store")
                    .body(Body::from(tile))
                    .expect("static response headers are valid"),
                Err(error) => text_response(StatusCode::BAD_GATEWAY, &format!("{error}\n")),
            }
        }
    }
}

fn response(status: StatusCode, body: impl Into<Body>, content_type: &'static str) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(body.into())
        .expect("static response headers are valid")
}

fn text_response(status: StatusCode, body: &str) -> Response {
    response(
        status,
        Body::from(body.to_owned()),
        "text/plain; charset=utf-8",
    )
}

fn tile_coordinates(path: &str) -> Option<(&str, &str, &str)> {
    let path = path.strip_prefix("/tiles/")?.strip_suffix(".pbf")?;
    let mut parts = path.split('/');
    let coordinates = (parts.next()?, parts.next()?, parts.next()?);
    parts.next().is_none().then_some(coordinates)
}
