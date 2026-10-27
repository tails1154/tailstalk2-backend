use axum::{
    extract::{Multipart, Path},
    http::header,
    response::{Response, IntoResponse},
    Json,
};
use revolt_database::User;
use revolt_result::{create_error, Result};
use serde::Serialize;

const MAX_GIF_SIZE: usize = 20 * 1024 * 1024;

#[derive(Serialize)]
pub struct UploadResponse {
    pub url: String,
}

pub async fn upload(_user: User, mut multipart: Multipart) -> Result<Json<UploadResponse>> {
    let mut contents = None;

    while let Some(field) = multipart.next_field().await.map_err(|_| create_error!(InvalidOperation))? {
        if field.name() != Some("file") {
            continue;
        }

        if field.content_type() != Some("image/gif") {
            return Err(create_error!(InvalidOperation));
        }

        let bytes = field.bytes().await.map_err(|_| create_error!(InvalidOperation))?;
        if bytes.is_empty() {
            return Err(create_error!(InvalidOperation));
        }
        if bytes.len() > MAX_GIF_SIZE {
            return Err(create_error!(FileTooLarge { max: MAX_GIF_SIZE }));
        }

        contents = Some(bytes);
        break;
    }

    let contents = contents.ok_or(create_error!(InvalidOperation))?;
    let id = ulid::Ulid::new().to_string();
    let directory = std::path::Path::new("/data/gifs");
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|_| create_error!(InternalError))?;
    tokio::fs::write(directory.join(format!("{id}.gif")), contents)
        .await
        .map_err(|_| create_error!(InternalError))?;

    Ok(Json(UploadResponse {
        url: format!("/media/{id}.gif"),
    }))
}

pub async fn media(Path(filename): Path<String>) -> Result<Response> {
    if !filename.ends_with(".gif")
        || filename.len() < 5
        || !filename[..filename.len() - 4]
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(create_error!(InvalidOperation));
    }

    let path = std::path::Path::new("/data/gifs").join(&filename);
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| create_error!(NotFound))?;
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, header::HeaderValue::from_static("image/gif"));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, header::HeaderValue::from_static("public, max-age=31536000, immutable"));
    Ok(response)
}
