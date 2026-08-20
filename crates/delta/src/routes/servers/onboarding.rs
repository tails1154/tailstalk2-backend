use revolt_database::{
    mongodb::bson::{doc, Document},
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    Database, User,
};
use revolt_permissions::{calculate_server_permissions, ChannelPermission};
use revolt_result::{create_database_error, Result};
use rocket::{serde::json::Json, State};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ServerOnboarding {
    pub enabled: bool,
    pub title: String,
    pub message: String,
    pub rules: String,
}

/// Fetch onboarding settings for a server member.
#[openapi(tag = "Server Information")]
#[get("/<target>/onboarding")]
pub async fn fetch(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<ServerOnboarding>> {
    let server = target.as_server(db).await?;
    db.fetch_member(&server.id, &user.id).await?;

    let settings = match &**db {
        Database::MongoDb(mongo) => mongo
            .col::<ServerOnboarding>("server_onboarding")
            .find_one(doc! { "_id": &server.id })
            .await
            .map_err(|_| create_database_error!("find_one", "server_onboarding"))?
            .unwrap_or_else(|| ServerOnboarding {
                enabled: false,
                title: format!("Welcome to {}", server.name),
                ..Default::default()
            }),
        _ => ServerOnboarding {
            enabled: false,
            title: format!("Welcome to {}", server.name),
            ..Default::default()
        },
    };

    Ok(Json(settings))
}

/// Update onboarding settings. Requires ManageServer.
#[openapi(tag = "Server Information")]
#[patch("/<target>/onboarding", data = "<data>")]
pub async fn update(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<ServerOnboarding>,
) -> Result<Json<ServerOnboarding>> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;

    let settings = data.into_inner();
    match &**db {
        Database::MongoDb(mongo) => {
            mongo
                .col::<Document>("server_onboarding")
                .replace_one(
                    doc! { "_id": &server.id },
                    doc! {
                        "_id": &server.id,
                        "enabled": settings.enabled,
                        "title": &settings.title,
                        "message": &settings.message,
                        "rules": &settings.rules,
                    },
                )
                .upsert(true)
                .await
                .map_err(|_| create_database_error!("replace_one", "server_onboarding"))?;
        }
        _ => return Err(create_database_error!("replace_one", "server_onboarding")),
    }

    Ok(Json(settings))
}
