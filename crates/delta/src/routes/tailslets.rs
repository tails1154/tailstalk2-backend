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
pub struct Tailslet {
    pub id: String,
    pub author_id: String,
    pub author_name: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TailsletSubmission {
    pub content: String,
}

fn from_doc(document: &Document) -> Option<Tailslet> {
    Some(Tailslet {
        id: document.get_str("_id").ok()?.to_owned(),
        author_id: document.get_str("author_id").ok()?.to_owned(),
        author_name: document.get_str("author_name").ok()?.to_owned(),
        content: document.get_str("content").ok()?.to_owned(),
        created_at: document.get_str("created_at").ok()?.to_owned(),
    })
}

#[openapi(tag = "Core")]
#[get("/tailslets?<before>&<limit>")]
pub async fn list(
    db: &State<Database>,
    _user: User,
    before: Option<String>,
    limit: Option<u8>,
) -> Result<Json<Vec<Tailslet>>> {
    let page_size = i64::from(limit.unwrap_or(20).clamp(1, 20));
    let filter = before
        .map(|cursor| doc! { "created_at": { "$lt": cursor } })
        .unwrap_or_default();
    if let Database::MongoDb(mongo) = db.inner() {
        use futures::StreamExt;
        let posts = mongo
            .col::<Document>("user_tailslets")
            .find(filter)
            .sort(doc! { "created_at": -1 })
            .limit(page_size)
            .await
            .map_err(|_| create_error!(InternalError))?
            .filter_map(|item| async move { item.ok().and_then(|document| from_doc(&document)) })
            .collect()
            .await;
        Ok(Json(posts))
    } else {
        Ok(Json(Vec::new()))
    }
}

#[openapi(tag = "User Information")]
#[post("/tailslets", data = "<data>")]
pub async fn create(
    db: &State<Database>,
    user: User,
    data: Json<TailsletSubmission>,
) -> Result<Json<Tailslet>> {
    let content = data.into_inner().content.trim().to_owned();
    if content.is_empty() || content.chars().count() > 2000 {
        return Err(create_error!(FailedValidation {
            error: "Tailslet content is invalid.".to_owned()
        }));
    }
    let post = Tailslet {
        id: Ulid::new().to_string(),
        author_id: user.id,
        author_name: user.username,
        content,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Database::MongoDb(mongo) = db.inner() {
        let mut document = bson::to_document(&post).map_err(|_| create_error!(InternalError))?;
        document.insert("_id", &post.id);
        mongo
            .col::<Document>("user_tailslets")
            .insert_one(document)
            .await
            .map_err(|_| create_error!(InternalError))?;
        Ok(Json(post))
    } else {
        Err(create_error!(InternalError))
    }
}

#[openapi(tag = "User Information")]
#[delete("/tailslets/<id>")]
pub async fn delete(db: &State<Database>, user: User, id: String) -> Result<()> {
    if let Database::MongoDb(mongo) = db.inner() {
        let result = mongo
            .col::<Document>("user_tailslets")
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
