use revolt_database::{mongodb::bson::{self, doc, Document}, Database, User};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

const CONFIG_ID: &str = "feature_requests";

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FeatureRequest {
    pub id: String,
    pub title: String,
    pub body: String,
    pub author_id: String,
    pub author_name: String,
    pub status: String,
    pub admin_response: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FeatureRequestSubmission { pub title: String, pub body: String }

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FeatureRequestDecision { pub response: Option<String> }

fn from_doc(document: &Document) -> Option<FeatureRequest> {
    Some(FeatureRequest {
        id: document.get_str("id").ok()?.to_owned(),
        title: document.get_str("title").ok()?.to_owned(),
        body: document.get_str("body").ok()?.to_owned(),
        author_id: document.get_str("author_id").ok()?.to_owned(),
        author_name: document.get_str("author_name").unwrap_or_default().to_owned(),
        status: document.get_str("status").unwrap_or("pending").to_owned(),
        admin_response: document.get_str("admin_response").unwrap_or_default().to_owned(),
    })
}

async fn load(db: &Database) -> Result<Vec<Document>> {
    match db {
        Database::MongoDb(mongo) => Ok(mongo.col::<Document>("platform_config")
            .find_one(doc! { "_id": CONFIG_ID }).await
            .map_err(|_| create_error!(InternalError))?
            .and_then(|document| document.get_array("entries").ok().cloned())
            .unwrap_or_default().into_iter()
            .filter_map(|value| value.as_document().cloned()).collect()),
        _ => Ok(Vec::new()),
    }
}

async fn save(db: &Database, entries: Vec<Document>) -> Result<()> {
    if let Database::MongoDb(mongo) = db {
        let entries = bson::to_bson(&entries).map_err(|_| create_error!(InternalError))?;
        mongo.col::<Document>("platform_config")
            .update_one(doc! { "_id": CONFIG_ID }, doc! { "$set": { "entries": entries } })
            .upsert(true).await.map_err(|_| create_error!(InternalError))?;
    }
    Ok(())
}

#[openapi(tag = "Core")]
#[get("/feature-requests")]
pub async fn list(db: &State<Database>, user: User) -> Result<Json<Vec<FeatureRequest>>> {
    Ok(Json(load(db.inner()).await?.iter().filter_map(from_doc)
        .filter(|request| request.author_id == user.id).collect()))
}

#[openapi(tag = "Core")]
#[post("/feature-requests", data = "<data>")]
pub async fn create(db: &State<Database>, user: User, data: Json<FeatureRequestSubmission>) -> Result<Json<FeatureRequest>> {
    let data = data.into_inner();
    if data.title.trim().is_empty() || data.title.chars().count() > 120 || data.body.trim().is_empty() || data.body.chars().count() > 4000 {
        return Err(create_error!(FailedValidation { error: "Feature request title or description is invalid.".to_owned() }));
    }
    let request = FeatureRequest { id: Ulid::new().to_string(), title: data.title.trim().to_owned(), body: data.body.trim().to_owned(), author_id: user.id, author_name: user.username, status: "pending".to_owned(), admin_response: String::new() };
    let mut entries = load(db.inner()).await?;
    entries.push(bson::to_document(&request).map_err(|_| create_error!(InternalError))?);
    save(db.inner(), entries).await?;
    Ok(Json(request))
}

#[openapi(tag = "Admin")]
#[get("/feature-requests")]
pub async fn admin_get(_admin: crate::routes::admin::auth::AdminUser, db: &State<Database>) -> Result<Json<Vec<FeatureRequest>>> {
    Ok(Json(load(db.inner()).await?.iter().filter_map(from_doc).collect()))
}

async fn decide(_admin: crate::routes::admin::auth::AdminUser, db: &State<Database>, id: String, status: &str, data: Option<Json<FeatureRequestDecision>>) -> Result<Json<FeatureRequest>> {
    let mut entries = load(db.inner()).await?;
    let document = entries.iter_mut().find(|entry| entry.get_str("id") == Ok(id.as_str())).ok_or(create_error!(NotFound))?;
    document.insert("status", status);
    document.insert("admin_response", data.and_then(|value| value.into_inner().response).unwrap_or_default());
    let request = from_doc(document).ok_or(create_error!(InternalError))?;
    save(db.inner(), entries).await?;
    Ok(Json(request))
}

#[openapi(tag = "Admin")]
#[post("/feature-requests/<id>/approve", data = "<data>")]
pub async fn approve(admin: crate::routes::admin::auth::AdminUser, db: &State<Database>, id: String, data: Option<Json<FeatureRequestDecision>>) -> Result<Json<FeatureRequest>> { decide(admin, db, id, "approved", data).await }

#[openapi(tag = "Admin")]
#[post("/feature-requests/<id>/deny", data = "<data>")]
pub async fn deny(admin: crate::routes::admin::auth::AdminUser, db: &State<Database>, id: String, data: Option<Json<FeatureRequestDecision>>) -> Result<Json<FeatureRequest>> { decide(admin, db, id, "denied", data).await }
