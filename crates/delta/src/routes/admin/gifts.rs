use revolt_database::{mongodb::bson::{doc, DateTime}, Database};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use super::auth::AdminUser;

#[derive(Deserialize, JsonSchema)]
pub struct CreateGiftData {
    pub duration_days: i64,
}

#[derive(Serialize, JsonSchema)]
pub struct CreatedGift {
    pub id: String,
    pub url: String,
    pub duration_days: i64,
}

#[openapi(tag = "Admin")]
#[post("/gifts", data = "<data>")]
pub async fn admin_create_gift(
    _admin: AdminUser,
    db: &State<Database>,
    data: Json<CreateGiftData>,
) -> Result<Json<CreatedGift>> {
    if !(1..=3650).contains(&data.duration_days) {
        return Err(create_error!(FailedValidation {
            error: "Gift duration must be between 1 and 3650 days".to_owned()
        }));
    }
    let Database::MongoDb(mongo) = db.inner() else {
        return Err(create_error!(InternalError));
    };
    let id = Ulid::new().to_string();
    mongo.col::<revolt_database::mongodb::bson::Document>("premium_gifts")
        .insert_one(doc! { "_id": &id, "duration_days": data.duration_days, "created_at": DateTime::now(), "revoked": false })
        .await
        .map_err(|_| create_error!(InternalError))?;
    Ok(Json(CreatedGift {
        url: format!("https://tailstalk2.com/gift/{id}"),
        id,
        duration_days: data.duration_days,
    }))
}
