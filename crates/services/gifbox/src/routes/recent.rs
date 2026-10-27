use axum::{extract::Query, Json};
use revolt_database::User;
use revolt_result::Result;
use serde::Deserialize;

use crate::types::{MediaObject, MediaResult, PaginatedMediaResponse};
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct RecentQuery {
    pub limit: Option<usize>,
}

pub async fn recent(
    _user: User,
    Query(query): Query<RecentQuery>,
) -> Result<Json<PaginatedMediaResponse>> {
    let limit = query.limit.unwrap_or(50).min(100);
    let mut filenames = Vec::new();
    let mut entries = tokio::fs::read_dir("/data/gifs")
        .await
        .map_err(|_| revolt_result::create_error!(InternalError))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|_| revolt_result::create_error!(InternalError))?
    {
        let filename = entry.file_name().to_string_lossy().into_owned();
        if filename.ends_with(".gif") {
            filenames.push(filename);
        }
    }

    filenames.sort_by(|left, right| right.cmp(left));
    let results = filenames
        .into_iter()
        .take(limit)
        .map(|filename| {
            let url = format!("/media/{filename}");
            let mut media_formats = HashMap::new();
            media_formats.insert(
                "gif".to_owned(),
                MediaObject {
                    url: url.clone(),
                    dimensions: Vec::new(),
                },
            );
            MediaResult {
                id: filename.trim_end_matches(".gif").to_owned(),
                media_formats,
                url,
            }
        })
        .collect();

    Ok(Json(PaginatedMediaResponse { results, next: None }))
}
