use crate::AppState;
use axum::{
    extract::DefaultBodyLimit,
    http::Method,
    routing::{get, post, Router},
};
use tower_http::cors::{AllowHeaders, Any, CorsLayer};

pub mod categories;
pub mod root;
pub mod recent;
pub mod search;
pub mod trending;
pub mod upload;

pub fn router() -> Router<AppState> {
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(AllowHeaders::mirror_request())
        .expose_headers(vec![
            "X-RateLimit-Limit".try_into().unwrap(),
            "X-RateLimit-Bucket".try_into().unwrap(),
            "X-RateLimit-Remaining".try_into().unwrap(),
            "X-RateLimit-Reset-After".try_into().unwrap(),
        ])
        .allow_origin(Any);

    Router::new()
        .route("/", get(root::root))
        .route("/recent", get(recent::recent))
        .route("/categories", get(categories::categories))
        .route("/search", get(search::search))
        .route("/trending", get(trending::trending))
        .route(
            "/upload",
            post(upload::upload)
                .layer(DefaultBodyLimit::max(25 * 1024 * 1024)),
        )
        .route("/media/:filename", get(upload::media))
        .layer(cors)
}
