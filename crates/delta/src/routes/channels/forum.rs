use futures::StreamExt;
use revolt_database::{
    mongodb::bson::{doc, DateTime, Document},
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    Database, User,
};
use revolt_permissions::{calculate_channel_permissions, ChannelPermission};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const COLLECTION: &str = "forum_post_metadata";

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ForumPostMetadata {
    pub message_id: String,
    pub channel_id: String,
    pub tags: Vec<String>,
    pub solved: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ForumPostMetadataUpdate {
    pub tags: Option<Vec<String>>,
    pub solved: Option<bool>,
}

fn from_document(document: Document) -> Option<ForumPostMetadata> {
    Some(ForumPostMetadata {
        message_id: document.get_str("message_id").ok()?.to_owned(),
        channel_id: document.get_str("channel_id").ok()?.to_owned(),
        tags: document
            .get_array("tags")
            .ok()?
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        solved: document.get_bool("solved").unwrap_or(false),
    })
}

fn validate_tags(tags: &[String]) -> bool {
    tags.len() <= 5
        && tags.iter().all(|tag| {
            let trimmed = tag.trim();
            !trimmed.is_empty() && trimmed.chars().count() <= 32
        })
}

async fn mongo(db: &State<Database>) -> Result<&revolt_database::MongoDb> {
    match db.inner() {
        Database::MongoDb(mongo) => Ok(mongo),
        _ => Err(create_error!(InternalError)),
    }
}

#[openapi(tag = "Messaging")]
#[get("/<target>/forum")]
pub async fn fetch(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<Vec<ForumPostMetadata>>> {
    let channel = target.as_channel(db).await?;
    let mut permissions = DatabasePermissionQuery::new(db, &user).channel(&channel);
    calculate_channel_permissions(&mut permissions)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ReadMessageHistory)?;

    let mut cursor = mongo(db)
        .await?
        .col::<Document>(COLLECTION)
        .find(doc! { "channel_id": channel.id() })
        .await
        .map_err(|_| create_error!(InternalError))?;
    let mut metadata = Vec::new();
    while let Some(document) = cursor.next().await {
        if let Ok(document) = document {
            if let Some(entry) = from_document(document) {
                metadata.push(entry);
            }
        }
    }
    Ok(Json(metadata))
}

#[openapi(tag = "Messaging")]
#[patch("/<target>/messages/<msg>/forum", data = "<update>")]
pub async fn update(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    msg: Reference<'_>,
    update: Json<ForumPostMetadataUpdate>,
) -> Result<Json<ForumPostMetadata>> {
    let channel = target.as_channel(db).await?;
    let mut permissions = DatabasePermissionQuery::new(db, &user).channel(&channel);
    let permissions = calculate_channel_permissions(&mut permissions).await;
    permissions.throw_if_lacking_channel_permission(ChannelPermission::ReadMessageHistory)?;

    let message = msg.as_message_in_channel(db, channel.id()).await?;
    let can_manage = permissions.has_channel_permission(ChannelPermission::ManageMessages);
    if message.author != user.id && !can_manage {
        return Err(create_error!(NotFound));
    }

    let update = update.into_inner();
    let mongo = mongo(db).await?;
    let existing = mongo
        .col::<Document>(COLLECTION)
        .find_one(doc! { "_id": message.id.as_str() })
        .await
        .map_err(|_| create_error!(InternalError))?;
    let current_tags = existing
        .as_ref()
        .and_then(|document| document.get_array("tags").ok())
        .map(|tags| {
            tags.iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let tags = update.tags.unwrap_or(current_tags);
    if !validate_tags(&tags) {
        return Err(create_error!(FailedValidation {
            error: "Forum tags must contain at most five names of 32 characters or fewer."
                .to_owned()
        }));
    }
    let solved = update
        .solved
        .or_else(|| {
            existing
                .as_ref()
                .and_then(|document| document.get_bool("solved").ok())
        })
        .unwrap_or(false);
    let entry = ForumPostMetadata {
        message_id: message.id.clone(),
        channel_id: channel.id().to_owned(),
        tags: tags
            .into_iter()
            .map(|tag| tag.trim().to_owned())
            .filter(|tag| !tag.is_empty())
            .collect(),
        solved,
    };
    let mut document = revolt_database::mongodb::bson::to_document(&entry)
        .map_err(|_| create_error!(InternalError))?;
    document.insert("_id", message.id.as_str());
    document.insert("updated_at", DateTime::now());
    mongo
        .col::<Document>(COLLECTION)
        .replace_one(doc! { "_id": message.id.as_str() }, document)
        .upsert(true)
        .await
        .map_err(|_| create_error!(InternalError))?;

    Ok(Json(entry))
}
