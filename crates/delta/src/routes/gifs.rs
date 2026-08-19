use revolt_result::{create_error, Result};
use rocket::serde::json::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const GIF_API: &str = "https://api.gifukai.com/v1";
const GIF_ACTIONS: &[&str] = &[
    "angry", "blush", "cry", "dance", "happy", "hug", "kiss", "laugh", "pat", "slap",
    "smile", "wave",
];

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GifResult {
    pub action: String,
    pub url: String,
    pub filename: String,
    pub content_type: String,
}

/// Fetch a random GIF from the no-key GIF provider.
#[openapi(tag = "Core")]
#[get("/gifs/<action>")]
pub async fn random(action: String) -> Result<Json<GifResult>> {
    if !GIF_ACTIONS.contains(&action.as_str()) {
        return Err(create_error!(InvalidOperation));
    }

    let result = reqwest::Client::new()
        .get(format!("{GIF_API}/{action}"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| create_error!(InternalError))?
        .error_for_status()
        .map_err(|_| create_error!(InternalError))?
        .json::<GifResult>()
        .await
        .map_err(|_| create_error!(InternalError))?;

    Ok(Json(result))
}
