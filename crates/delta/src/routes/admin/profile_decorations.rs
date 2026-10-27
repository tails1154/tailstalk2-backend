use revolt_config::config;
use revolt_database::{mongodb::bson::{doc, Document}, Database};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use super::auth::AdminUser;

const DECORATIONS: &str = "profile_decorations";

#[derive(Debug, Serialize, JsonSchema)]
pub struct AdminDecoration {
    pub id: String,
    pub name: String,
    pub asset_url: String,
    pub price_xp: i64,
    pub enabled: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateDecoration {
    pub name: String,
    pub asset_id: String,
    pub price_xp: i64,
}

fn mongo(db: &Database) -> Result<&revolt_database::MongoDb> {
    match db {
        Database::MongoDb(db) => Ok(db),
        _ => Err(create_error!(InternalError)),
    }
}

async fn asset_url(asset_id: &str) -> String {
    format!("{}/attachments/{}/original", config().await.hosts.autumn, asset_id)
}

async fn response(document: Document) -> Result<AdminDecoration> {
    let asset_id = document.get_str("asset_id").map_err(|_| create_error!(InternalError))?;
    Ok(AdminDecoration {
        id: document.get_str("_id").map_err(|_| create_error!(InternalError))?.to_owned(),
        name: document.get_str("name").unwrap_or("Decoration").to_owned(),
        asset_url: asset_url(asset_id).await,
        price_xp: document.get_i64("price_xp").unwrap_or(0),
        enabled: document.get_bool("enabled").unwrap_or(true),
    })
}

#[openapi(tag = "Admin")]
#[get("/profile-decorations")]
pub async fn list(_admin: AdminUser, db: &State<Database>) -> Result<Json<Vec<AdminDecoration>>> {
    use futures::TryStreamExt;
    let mut cursor = mongo(db)?.col::<Document>(DECORATIONS).find(doc! {}).await
        .map_err(|_| create_error!(InternalError))?;
    let mut output = Vec::new();
    while let Some(document) = cursor.try_next().await.map_err(|_| create_error!(InternalError))? {
        output.push(response(document).await?);
    }
    Ok(Json(output))
}

#[openapi(tag = "Admin")]
#[post("/profile-decorations", data = "<data>")]
pub async fn create(
    _admin: AdminUser,
    db: &State<Database>,
    data: Json<CreateDecoration>,
) -> Result<Json<AdminDecoration>> {
    if data.name.trim().is_empty() || data.name.len() > 64 || data.asset_id.is_empty() || data.price_xp < 0 {
        return Err(create_error!(FailedValidation { error: "Invalid decoration".to_owned() }));
    }
    let id = Ulid::new().to_string();
    mongo(db)?.col::<Document>(DECORATIONS).insert_one(doc! {
        "_id": &id,
        "name": data.name.trim(),
        "asset_id": &data.asset_id,
        "price_xp": data.price_xp,
        "enabled": true,
    }).await.map_err(|_| create_error!(InternalError))?;
    let document = mongo(db)?.col::<Document>(DECORATIONS).find_one(doc! { "_id": &id }).await
        .map_err(|_| create_error!(InternalError))?.ok_or(create_error!(InternalError))?;
    Ok(Json(response(document).await?))
}
