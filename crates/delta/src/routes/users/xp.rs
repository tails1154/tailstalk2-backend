use revolt_database::{
    mongodb::bson::{doc, Document},
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    Database, User,
};
use futures::StreamExt;
use revolt_permissions::{calculate_server_permissions, ChannelPermission};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MESSAGE_XP: i64 = 5;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct XPSettings {
    pub enabled: bool,
    pub xp_per_message: i64,
}

impl Default for XPSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            xp_per_message: MESSAGE_XP,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct XPSettingsUpdate {
    pub enabled: Option<bool>,
    pub xp_per_message: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct XPResponse {
    pub user_id: String,
    pub xp: i64,
    pub level: i64,
    pub next_level_xp: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct XPLeaderboardEntry {
    pub rank: i64,
    pub user_id: String,
    pub xp: i64,
    pub level: i64,
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

pub(crate) async fn read_settings(db: &Database, server_id: &str) -> XPSettings {
    match db {
        Database::MongoDb(mongo) => mongo
            .col::<Document>("server_xp")
            .find_one(doc! { "_id": server_id })
            .await
            .ok()
            .flatten()
            .map(|entry| XPSettings {
                enabled: entry.get_bool("enabled").unwrap_or(true),
                xp_per_message: entry
                    .get_i64("xp_per_message")
                    .unwrap_or(MESSAGE_XP)
                    .clamp(0, 20),
            })
            .unwrap_or_default(),
        _ => XPSettings::default(),
    }
}

/// Fetch a server's XP settings.
#[openapi(tag = "Server Information")]
#[get("/<target>/xp")]
pub async fn fetch_settings(
    db: &State<Database>,
    _user: User,
    target: Reference<'_>,
) -> Result<Json<XPSettings>> {
    let server = target.as_server(db).await?;
    Ok(Json(read_settings(db, &server.id).await))
}

/// Update a server's XP settings.
#[openapi(tag = "Server Information")]
#[patch("/<target>/xp", data = "<data>")]
pub async fn update_settings(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<XPSettingsUpdate>,
) -> Result<Json<XPSettings>> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    let permissions = calculate_server_permissions(&mut query).await;
    permissions.throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;

    let current = read_settings(db, &server.id).await;
    let settings = XPSettings {
        enabled: data.enabled.unwrap_or(current.enabled),
        xp_per_message: data
            .xp_per_message
            .unwrap_or(current.xp_per_message)
            .clamp(0, 20),
    };

    if let Database::MongoDb(mongo) = db.inner() {
        mongo
            .col::<Document>("server_xp")
            .update_one(
                doc! { "_id": &server.id },
                doc! {
                    "$set": {
                        "enabled": settings.enabled,
                        "xp_per_message": settings.xp_per_message,
                    }
                },
            )
            .upsert(true)
            .await
            .map_err(|_| create_error!(InternalError))?;
    }

    Ok(Json(settings))
}

/// Fetch the XP leaderboard for members of a server.
#[openapi(tag = "Server Information")]
#[get("/<target>/xp/leaderboard")]
pub async fn leaderboard(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<Vec<XPLeaderboardEntry>>> {
    let server = target.as_server(db).await?;
    db.fetch_member(&server.id, &user.id).await?;
    let member_ids = db
        .fetch_all_members(&server.id)
        .await?
        .into_iter()
        .map(|member| member.id.user)
        .collect::<Vec<_>>();
    let entries = match db.inner() {
        Database::MongoDb(mongo) if !member_ids.is_empty() => mongo
            .col::<Document>("user_xp")
            .find(doc! { "_id": { "$in": &member_ids } })
            .sort(doc! { "xp": -1_i32 })
            .limit(100)
            .await
            .map_err(|_| create_error!(InternalError))?
            .filter_map(|item| async move { item.ok() })
            .filter_map(|item| async move {
                Some((item.get_str("_id").ok()?.to_owned(), item.get_i64("xp").unwrap_or(0)))
            })
            .collect::<Vec<_>>()
            .await,
        _ => Vec::new(),
    };
    Ok(Json(entries.into_iter().enumerate().map(|(index, (user_id, xp))| XPLeaderboardEntry {
        rank: index as i64 + 1,
        user_id,
        xp,
        level: xp.div_euclid(100) + 1,
    }).collect()))
}

/// Fetch a user's XP and level.
#[openapi(tag = "User Information")]
#[get("/<target>/xp")]
pub async fn fetch(db: &State<Database>, _user: User, target: Reference<'_>) -> Result<Json<XPResponse>> {
    let target = target.as_user(db).await?;
    Ok(Json(read(db, &target.id).await?))
}

/// Award XP after a successful message send.
pub(crate) async fn award_message(db: &Database, user_id: &str, server_id: Option<&str>) {
    if let Database::MongoDb(mongo) = db {
        let amount = if let Some(server_id) = server_id {
            let settings = read_settings(db, server_id).await;
            if !settings.enabled {
                return;
            }
            settings.xp_per_message
        } else {
            MESSAGE_XP
        };

        let _ = mongo
            .col::<Document>("user_xp")
            .update_one(
                doc! { "_id": user_id },
                doc! { "$inc": { "xp": amount } },
            )
            .upsert(true)
            .await;
    }
}
