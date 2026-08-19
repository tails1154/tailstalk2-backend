use revolt_database::{mongodb::bson::{doc, Document}, util::reference::Reference, Database, User};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::Serialize;

pub const MESSAGE_XP: i64 = 5;

#[derive(Debug, Serialize, JsonSchema)]
pub struct XPResponse {
    pub user_id: String,
    pub xp: i64,
    pub level: i64,
    pub next_level_xp: i64,
}

fn response(user_id: String, xp: i64) -> XPResponse {
    let level = xp.div_euclid(100) + 1;
    XPResponse {
        user_id,
        xp,
        level,
        next_level_xp: level * 100,
    }
}

async fn read(db: &Database, user_id: &str) -> Result<XPResponse> {
    let xp = match db {
        Database::MongoDb(mongo) => mongo
            .col::<Document>("user_xp")
            .find_one(doc! { "_id": user_id })
            .await
            .map_err(|_| create_error!(InternalError))?
            .and_then(|entry| entry.get_i64("xp").ok())
            .unwrap_or(0),
        _ => 0,
    };
    Ok(response(user_id.to_string(), xp))
}

/// Fetch a user's XP and level.
#[openapi(tag = "User Information")]
#[get("/<target>/xp")]
pub async fn fetch(db: &State<Database>, _user: User, target: Reference<'_>) -> Result<Json<XPResponse>> {
    let target = target.as_user(db).await?;
    Ok(Json(read(db, &target.id).await?))
}

/// Award XP after a successful message send.
pub(crate) async fn award_message(db: &Database, user_id: &str) {
    if let Database::MongoDb(mongo) = db {
        let _ = mongo
            .col::<Document>("user_xp")
            .update_one(
                doc! { "_id": user_id },
                doc! { "$inc": { "xp": MESSAGE_XP } },
            )
            .upsert(true)
            .await;
    }
}
