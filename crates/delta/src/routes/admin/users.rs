use revolt_database::{
    mongodb::bson::{self, doc},
    Database,
};
use revolt_result::Result;
use rocket::State;
use rocket::serde::json::Json;
use serde::{Deserialize, Serialize};

use super::auth::AdminUser;

#[derive(Serialize, JsonSchema, Debug)]
pub struct AdminUserInfo {
    pub id: String,
    pub username: String,
    pub discriminator: String,
    pub display_name: Option<String>,
    pub flags: Option<i32>,
    pub suspended_until: Option<String>,
    pub privileged: bool,
    pub warnings: Vec<AdminWarning>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
pub struct AdminWarning {
    pub reason: String,
    pub created_at: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct WarnUserData {
    pub reason: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct SuspendUserData {
    pub hours: i64,
}

impl AdminWarning {
    fn from_bson(arr: &bson::Array) -> Vec<AdminWarning> {
        arr.iter()
            .filter_map(|v| {
                let doc = v.as_document()?;
                Some(AdminWarning {
                    reason: doc.get_str("reason").ok()?.to_string(),
                    created_at: doc.get_str("created_at").ok()?.to_string(),
                })
            })
            .collect()
    }
}

fn user_from_doc(doc: &bson::Document) -> Option<AdminUserInfo> {
    let suspended = doc
        .get_str("suspended_until")
        .ok()
        .map(|s| s.to_string());
    let display = doc.get_str("display_name").ok().map(|s| s.to_string());
    let warnings = doc
        .get_array("admin_warnings")
        .map(AdminWarning::from_bson)
        .unwrap_or_default();

    Some(AdminUserInfo {
        id: doc.get_str("_id").ok()?.to_string(),
        username: doc.get_str("username").ok()?.to_string(),
        discriminator: doc.get_str("discriminator").unwrap_or("0000").to_string(),
        display_name: display,
        flags: doc.get_i32("flags").ok(),
        suspended_until: suspended,
        privileged: doc.get_bool("privileged").unwrap_or(false),
        warnings,
    })
}

/// # Search Users
///
/// Search users by username. Requires HTTP Basic Auth.
#[openapi(tag = "Admin")]
#[get("/users/search?<q>")]
pub async fn admin_search_users(
    _admin: AdminUser,
    db: &State<Database>,
    q: String,
) -> Result<Json<Vec<AdminUserInfo>>> {
    let users = match db.inner() {
        Database::MongoDb(mongo) => {
            let mut cursor = mongo
                .col::<bson::Document>("users")
                .find(doc! {
                    "username": { "$regex": &q as &str, "$options": "i" }
                })
                .limit(20)
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?;

            let mut result = Vec::new();
            use futures::StreamExt;
            while let Some(Ok(doc)) = cursor.next().await {
                if let Some(user) = user_from_doc(&doc) {
                    result.push(user);
                }
            }
            result
        }
        _ => Vec::new(),
    };

    Ok(Json(users))
}

/// # Ban User
///
/// Ban a user permanently.
#[openapi(tag = "Admin")]
#[post("/users/<id>/ban")]
pub async fn admin_ban_user(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
) -> Result<Json<serde_json::Value>> {
    let far_future = iso8601_timestamp::Timestamp::now_utc()
        + iso8601_timestamp::Duration::days(36500i64);

    match db.inner() {
        Database::MongoDb(mongo) => {
            mongo
                .col::<bson::Document>("users")
                .update_one(
                    doc! { "_id": &id },
                    doc! {
                        "$set": {
                            "flags": 5i32,
                            "suspended_until": bson::to_bson(&far_future).unwrap_or_default(),
                        }
                    },
                )
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?;
        }
        _ => {}
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// # Unban User
///
/// Unban a user.
#[openapi(tag = "Admin")]
#[post("/users/<id>/unban")]
pub async fn admin_unban_user(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
) -> Result<Json<serde_json::Value>> {
    match db.inner() {
        Database::MongoDb(mongo) => {
            mongo
                .col::<bson::Document>("users")
                .update_one(
                    doc! { "_id": &id },
                    doc! {
                        "$set": { "flags": 0i32 },
                        "$unset": { "suspended_until": "" as &str },
                    },
                )
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?;
        }
        _ => {}
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// # Suspend User
///
/// Temporarily suspend a user for a number of hours.
#[openapi(tag = "Admin")]
#[post("/users/<id>/suspend", data = "<data>")]
pub async fn admin_suspend_user(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
    data: Json<SuspendUserData>,
) -> Result<Json<serde_json::Value>> {
    let until = iso8601_timestamp::Timestamp::now_utc()
        + iso8601_timestamp::Duration::hours(data.hours);

    match db.inner() {
        Database::MongoDb(mongo) => {
            mongo
                .col::<bson::Document>("users")
                .update_one(
                    doc! { "_id": &id },
                    doc! {
                        "$set": {
                            "flags": 1i32,
                            "suspended_until": bson::to_bson(&until).unwrap_or_default(),
                        }
                    },
                )
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?;
        }
        _ => {}
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// # Warn User
///
/// Add a warning to a user.
#[openapi(tag = "Admin")]
#[post("/users/<id>/warn", data = "<data>")]
pub async fn admin_warn_user(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
    data: Json<WarnUserData>,
) -> Result<Json<serde_json::Value>> {
    match db.inner() {
        Database::MongoDb(mongo) => {
            let reason: &str = &data.reason;
            mongo
                .col::<bson::Document>("users")
                .update_one(
                    doc! { "_id": &id },
                    doc! {
                        "$push": {
                            "admin_warnings": bson::to_bson(&bson::doc! {
                                "reason": reason,
                                "created_at": chrono::Utc::now().to_rfc3339(),
                            }).unwrap_or_default(),
                        }
                    },
                )
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?;
        }
        _ => {}
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// # Clear Warnings
///
/// Clear all warnings from a user.
#[openapi(tag = "Admin")]
#[post("/users/<id>/clear-warnings")]
pub async fn admin_clear_warnings(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
) -> Result<Json<serde_json::Value>> {
    match db.inner() {
        Database::MongoDb(mongo) => {
            mongo
                .col::<bson::Document>("users")
                .update_one(
                    doc! { "_id": &id },
                    doc! {
                        "$set": {
                            "admin_warnings": bson::to_bson(
                                &Vec::<bson::Document>::new()
                            ).unwrap_or_default()
                        }
                    },
                )
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?;
        }
        _ => {}
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// # Delete Warning
///
/// Delete a specific warning by index.
#[openapi(tag = "Admin")]
#[post("/users/<id>/delete-warning/<index>")]
pub async fn admin_delete_warning(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
    index: usize,
) -> Result<Json<serde_json::Value>> {
    match db.inner() {
        Database::MongoDb(mongo) => {
            // Fetch current warnings
            if let Ok(Some(user_doc)) = mongo
                .col::<bson::Document>("users")
                .find_one(doc! { "_id": &id })
                .await
            {
                if let Ok(arr) = user_doc.get_array("admin_warnings") {
                    if index < arr.len() {
                        let mut new_warnings: Vec<bson::Bson> = arr.iter().cloned().collect();
                        new_warnings.remove(index);
                        mongo
                            .col::<bson::Document>("users")
                            .update_one(
                                doc! { "_id": &id },
                                doc! {
                                    "$set": {
                                        "admin_warnings": bson::to_bson(&new_warnings)
                                            .unwrap_or_default(),
                                    }
                                },
                            )
                            .await
                            .map_err(|_| revolt_result::create_error!(InternalError))?;
                    }
                }
            }
        }
        _ => {}
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}
