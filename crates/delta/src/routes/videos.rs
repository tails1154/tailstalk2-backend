use revolt_database::{
    mongodb::bson::{self, doc, Document},
    Database, User,
};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct VideoPost {
    pub id: String,
    pub author_id: String,
    pub author_name: String,
    pub attachment_id: String,
    pub caption: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VideoSubmission {
    pub attachment_id: String,
    pub caption: String,
}

fn from_doc(document: &Document) -> Option<VideoPost> {
    Some(VideoPost {
        id: document.get_str("_id").ok()?.to_owned(),
        author_id: document.get_str("author_id").ok()?.to_owned(),
        author_name: document.get_str("author_name").ok()?.to_owned(),
        attachment_id: document.get_str("attachment_id").ok()?.to_owned(),
        caption: document.get_str("caption").unwrap_or_default().to_owned(),
        created_at: document.get_str("created_at").ok()?.to_owned(),
    })
}

#[openapi(tag = "Core")]
#[get("/videos?<before>&<limit>")]
pub async fn list(
    db: &State<Database>,
    _user: User,
    before: Option<String>,
    limit: Option<u8>,
) -> Result<Json<Vec<VideoPost>>> {
    let page_size = i64::from(limit.unwrap_or(20).clamp(1, 20));
    let filter = before
        .as_deref()
        .map(|cursor| doc! { "created_at": { "$lt": cursor } })
        .unwrap_or_else(|| doc! {});
    let videos = match db.inner() {
        Database::MongoDb(mongo) => {
            use futures::StreamExt;
            mongo
                .col::<Document>("user_videos")
                .find(filter)
                .sort(doc! { "created_at": -1 })
                .limit(page_size)
                .await
                .map_err(|_| create_error!(InternalError))?
                .filter_map(
                    |item| async move { item.ok().and_then(|document| from_doc(&document)) },
                )
                .collect::<Vec<_>>()
                .await
        }
        _ => Vec::new(),
    };

    Ok(Json(videos))
}

#[openapi(tag = "User Information")]
#[post("/videos", data = "<data>")]
pub async fn create(
    db: &State<Database>,
    user: User,
    data: Json<VideoSubmission>,
) -> Result<Json<VideoPost>> {
    let data = data.into_inner();
    if data.attachment_id.trim().is_empty()
        || data.attachment_id.len() > 128
        || data.caption.chars().count() > 500
    {
        return Err(create_error!(FailedValidation {
            error: "Video attachment or caption is invalid.".to_owned()
        }));
    }

    let post = VideoPost {
        id: Ulid::new().to_string(),
        author_id: user.id,
        author_name: user.username,
        attachment_id: data.attachment_id.trim().to_owned(),
        caption: data.caption.trim().to_owned(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    if let Database::MongoDb(mongo) = db.inner() {
        let mut document = bson::to_document(&post).map_err(|_| create_error!(InternalError))?;
        document.insert("_id", &post.id);
        mongo
            .col::<Document>("user_videos")
            .insert_one(document)
            .await
            .map_err(|_| create_error!(InternalError))?;
    } else {
        return Err(create_error!(InternalError));
    }

    Ok(Json(post))
}

/// Remove a video published by the authenticated user.
#[openapi(tag = "User Information")]
#[delete("/videos/<id>")]
pub async fn delete(db: &State<Database>, user: User, id: String) -> Result<()> {
    if let Database::MongoDb(mongo) = db.inner() {
        let result = mongo
            .col::<Document>("user_videos")
            .delete_one(doc! { "_id": id, "author_id": user.id })
            .await
            .map_err(|_| create_error!(InternalError))?;
        if result.deleted_count == 0 {
            return Err(create_error!(NotFound));
        }
        Ok(())
    } else {
        Err(create_error!(InternalError))
    }
}
