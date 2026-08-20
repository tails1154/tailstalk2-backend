use futures::StreamExt;
use revolt_database::{mongodb::bson::{self, doc, Document}, Database};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::routes::admin::auth::AdminUser;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Decoration {
    pub id: String,
    pub name: String,
    pub image: String,
    pub required_level: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DecorationCreate {
    pub name: String,
    pub image: String,
    pub required_level: Option<i64>,
}

fn from_doc(value: &Document) -> Option<Decoration> {
    Some(Decoration {
        id: value.get_str("_id").ok()?.to_owned(),
        name: value.get_str("name").ok()?.to_owned(),
        image: value.get_str("image").ok()?.to_owned(),
        required_level: value.get_i64("required_level").unwrap_or(1),
    })
}

async fn find(db: &Database, id: &str) -> Result<Decoration> {
    match db {
        Database::MongoDb(mongo) => mongo
            .col::<Document>("profile_decorations")
            .find_one(doc! { "_id": id })
            .await
            .map_err(|_| create_error!(InternalError))?
            .and_then(|value| from_doc(&value))
            .ok_or(create_error!(NotFound)),
        _ => Err(create_error!(NotFound)),
    }
}

/// List decorations available for profiles.
#[openapi(tag = "Core")]
#[get("/decorations")]
pub async fn list(db: &State<Database>) -> Result<Json<Vec<Decoration>>> {
    let entries = match db.inner() {
        Database::MongoDb(mongo) => mongo
            .col::<Document>("profile_decorations")
            .find(doc! {})
            .await
            .map_err(|_| create_error!(InternalError))?
            .filter_map(|value| async move { value.ok().and_then(|value| from_doc(&value)) })
            .collect::<Vec<_>>()
            .await,
        _ => Vec::new(),
    };
    Ok(Json(entries))
}

pub(crate) async fn validate_selection(db: &Database, id: &str, user_id: &str) -> Result<()> {
    let decoration = find(db, id).await?;
    if decoration.required_level <= 1 {
        return Ok(());
    }
    let xp = match db {
        Database::MongoDb(mongo) => mongo
            .col::<Document>("user_xp")
            .find_one(doc! { "_id": user_id })
            .await
            .map_err(|_| create_error!(InternalError))?
            .and_then(|value| value.get_i64("xp").ok())
            .unwrap_or(0),
        _ => 0,
    };
    if xp.div_euclid(100) + 1 < decoration.required_level {
        return Err(create_error!(NotPrivileged));
    }
    Ok(())
}

/// Create a profile decoration. Requires administrator authentication.
#[openapi(tag = "Admin")]
#[post("/decorations", data = "<data>")]
pub async fn admin_create(
    _admin: AdminUser,
    db: &State<Database>,
    data: Json<DecorationCreate>,
) -> Result<Json<Decoration>> {
    let data = data.into_inner();
    let name = data.name.trim();
    let required_level = data.required_level.unwrap_or(1).clamp(1, 10000);
    if name.is_empty() || name.chars().count() > 64 || !data.image.starts_with("data:image/") || data.image.len() > 3_000_000 {
        return Err(create_error!(FailedValidation { error: "Decoration fields are invalid.".to_owned() }));
    }
    let decoration = Decoration { id: Ulid::new().to_string(), name: name.to_owned(), image: data.image, required_level };
    if let Database::MongoDb(mongo) = db.inner() {
        mongo.col::<Document>("profile_decorations").insert_one(bson::to_document(&decoration).map_err(|_| create_error!(InternalError))?).await.map_err(|_| create_error!(InternalError))?;
    }
    Ok(Json(decoration))
}

/// Delete a profile decoration. Requires administrator authentication.
#[openapi(tag = "Admin")]
#[delete("/decorations/<id>")]
pub async fn admin_delete(_admin: AdminUser, db: &State<Database>, id: String) -> Result<()> {
    if let Database::MongoDb(mongo) = db.inner() {
        let result = mongo.col::<Document>("profile_decorations").delete_one(doc! { "_id": id }).await.map_err(|_| create_error!(InternalError))?;
        if result.deleted_count == 0 { return Err(create_error!(NotFound)); }
    }
    Ok(())
}
