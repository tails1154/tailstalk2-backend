use futures::TryStreamExt;
use revolt_config::config;
use revolt_database::{mongodb::bson::{doc, Document}, Database, PartialUser, User};
use revolt_models::v0::UserBadges;
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use ulid::Ulid;

const DECORATIONS: &str = "profile_decorations";
const OWNERSHIP: &str = "profile_decoration_ownership";

#[derive(Debug, Serialize, JsonSchema)]
pub struct DecorationResponse {
    pub id: String,
    pub name: String,
    pub asset_url: String,
    pub price_xp: i64,
    pub enabled: bool,
    pub owned: bool,
    pub equipped: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateDecoration {
    pub name: String,
    pub asset_id: String,
    pub price_xp: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateDecoration {
    pub name: Option<String>,
    pub asset_id: Option<String>,
    pub price_xp: Option<i64>,
    pub enabled: Option<bool>,
}

fn moderator(user: &User) -> bool {
    user.privileged || user.badges.unwrap_or_default() & UserBadges::PlatformModeration as i32 != 0
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

async fn response(db: &Database, user: &User, document: Document) -> Result<DecorationResponse> {
    let id = document.get_str("_id").map_err(|_| create_error!(InternalError))?;
    let name = document.get_str("name").unwrap_or("Decoration").to_owned();
    let asset_id = document.get_str("asset_id").map_err(|_| create_error!(InternalError))?;
    let owned = mongo(db)?.col::<Document>(OWNERSHIP)
        .find_one(doc! { "user_id": &user.id, "decoration_id": id })
        .await.map_err(|_| create_error!(InternalError))?.is_some();
    let equipped = user.profile.as_ref().and_then(|profile| profile.avatar_decoration.as_ref())
        == Some(&asset_url(asset_id).await);
    Ok(DecorationResponse {
        id: id.to_owned(),
        name,
        asset_url: asset_url(asset_id).await,
        price_xp: document.get_i64("price_xp").unwrap_or(0),
        enabled: document.get_bool("enabled").unwrap_or(true),
        owned,
        equipped,
    })
}

/// List enabled avatar decorations and the current user's ownership.
#[openapi(tag = "Profile Decorations")]
#[get("/profile-decorations")]
pub async fn list(db: &State<Database>, user: User) -> Result<Json<Vec<DecorationResponse>>> {
    let mut cursor = mongo(db)?.col::<Document>(DECORATIONS)
        .find(doc! { "enabled": true }).await.map_err(|_| create_error!(InternalError))?;
    let mut output = Vec::new();
    while let Some(document) = cursor.try_next().await.map_err(|_| create_error!(InternalError))? {
        output.push(response(db, &user, document).await?);
    }
    Ok(Json(output))
}

/// Spend XP to buy an avatar decoration.
#[openapi(tag = "Profile Decorations")]
#[post("/profile-decorations/<decoration_id>/purchase")]
pub async fn purchase(db: &State<Database>, user: User, decoration_id: String) -> Result<Json<serde_json::Value>> {
    if user.bot.is_some() { return Err(create_error!(IsBot)); }
    let database = mongo(db)?;
    let decoration = database.col::<Document>(DECORATIONS).find_one(doc! { "_id": &decoration_id, "enabled": true })
        .await.map_err(|_| create_error!(InternalError))?.ok_or(create_error!(NotFound))?;
    if database.col::<Document>(OWNERSHIP).find_one(doc! { "user_id": &user.id, "decoration_id": &decoration_id })
        .await.map_err(|_| create_error!(InternalError))?.is_some() {
        return Err(create_error!(FailedValidation { error: "Decoration already owned".to_owned() }));
    }
    let price = decoration.get_i64("price_xp").unwrap_or(0).max(0);
    let changed = database.col::<Document>("user_xp").update_one(
        doc! { "_id": &user.id, "xp": { "$gte": price } },
        doc! { "$inc": { "xp": -price } },
    ).await.map_err(|_| create_error!(InternalError))?;
    if changed.modified_count != 1 { return Err(create_error!(FailedValidation { error: "Not enough XP".to_owned() })); }
    database.col::<Document>(OWNERSHIP).insert_one(doc! {
        "_id": Ulid::new().to_string(), "user_id": &user.id, "decoration_id": &decoration_id,
    }).await.map_err(|_| create_error!(InternalError))?;
    Ok(Json(json!({ "ok": true })))
}

/// Equip a decoration the user owns.
#[openapi(tag = "Profile Decorations")]
#[post("/profile-decorations/<decoration_id>/equip")]
pub async fn equip(db: &State<Database>, mut user: User, decoration_id: String) -> Result<Json<serde_json::Value>> {
    let database = mongo(db)?;
    if database.col::<Document>(OWNERSHIP).find_one(doc! { "user_id": &user.id, "decoration_id": &decoration_id })
        .await.map_err(|_| create_error!(InternalError))?.is_none() { return Err(create_error!(NotFound)); }
    let decoration = database.col::<Document>(DECORATIONS).find_one(doc! { "_id": &decoration_id, "enabled": true })
        .await.map_err(|_| create_error!(InternalError))?.ok_or(create_error!(NotFound))?;
    let mut profile = user.profile.take().unwrap_or_default();
    profile.avatar_decoration = Some(asset_url(decoration.get_str("asset_id").map_err(|_| create_error!(InternalError))?).await);
    user.update(db, PartialUser { profile: Some(profile), ..Default::default() }, vec![]).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Remove the currently equipped decoration.
#[openapi(tag = "Profile Decorations")]
#[delete("/profile-decorations/equip")]
pub async fn unequip(db: &State<Database>, mut user: User) -> Result<Json<serde_json::Value>> {
    let mut profile = user.profile.take().unwrap_or_default();
    profile.avatar_decoration = None;
    user.update(db, PartialUser { profile: Some(profile), ..Default::default() }, vec![]).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Create a decoration from an Autumn attachment. Moderators may upload the attachment through the normal file picker first.
#[openapi(tag = "Profile Decorations")]
#[post("/profile-decorations", data = "<data>")]
pub async fn create(db: &State<Database>, user: User, data: Json<CreateDecoration>) -> Result<Json<DecorationResponse>> {
    if !moderator(&user) { return Err(create_error!(NotPrivileged)); }
    if data.name.trim().is_empty() || data.name.len() > 64 || data.asset_id.is_empty() || data.price_xp < 0 {
        return Err(create_error!(FailedValidation { error: "Invalid decoration".to_owned() }));
    }
    let id = Ulid::new().to_string();
    mongo(db)?.col::<Document>(DECORATIONS).insert_one(doc! {
        "_id": &id, "name": data.name.trim(), "asset_id": &data.asset_id, "price_xp": data.price_xp,
        "enabled": true, "created_by": &user.id,
    }).await.map_err(|_| create_error!(InternalError))?;
    let document = mongo(db)?.col::<Document>(DECORATIONS).find_one(doc! { "_id": &id }).await
        .map_err(|_| create_error!(InternalError))?.ok_or(create_error!(InternalError))?;
    Ok(Json(response(db, &user, document).await?))
}

/// Update a decoration's name, image, price, or availability.
#[openapi(tag = "Profile Decorations")]
#[patch("/profile-decorations/<decoration_id>", data = "<data>")]
pub async fn update(db: &State<Database>, user: User, decoration_id: String, data: Json<UpdateDecoration>) -> Result<Json<serde_json::Value>> {
    if !moderator(&user) { return Err(create_error!(NotPrivileged)); }
    let mut set = Document::new();
    if let Some(name) = &data.name { if name.trim().is_empty() || name.len() > 64 { return Err(create_error!(FailedValidation { error: "Invalid name".to_owned() })); } set.insert("name", name.trim()); }
    if let Some(asset_id) = &data.asset_id { set.insert("asset_id", asset_id); }
    if let Some(price_xp) = data.price_xp { if price_xp < 0 { return Err(create_error!(FailedValidation { error: "Invalid price".to_owned() })); } set.insert("price_xp", price_xp); }
    if let Some(enabled) = data.enabled { set.insert("enabled", enabled); }
    if !set.is_empty() { mongo(db)?.col::<Document>(DECORATIONS).update_one(doc! { "_id": &decoration_id }, doc! { "$set": set }).await.map_err(|_| create_error!(InternalError))?; }
    Ok(Json(json!({ "ok": true })))
}
