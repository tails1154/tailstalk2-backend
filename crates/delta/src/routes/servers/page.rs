use revolt_database::{
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    Database, User,
};
use revolt_permissions::{calculate_server_permissions, ChannelPermission};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ServerPageLink {
    pub label: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ServerPage {
    pub enabled: bool,
    pub title: String,
    pub description: String,
    pub links: Vec<ServerPageLink>,
}

fn valid(page: &ServerPage) -> bool {
    page.title.chars().count() <= 120
        && page.description.chars().count() <= 2000
        && page.links.len() <= 12
        && page.links.iter().all(|link| {
            !link.label.trim().is_empty()
                && link.label.chars().count() <= 64
                && (link.url.starts_with("https://") || link.url.starts_with("http://"))
                && link.url.chars().count() <= 512
        })
}

async fn read(db: &Database, server_id: &str) -> Result<ServerPage> {
    match db {
        Database::MongoDb(mongo) => Ok(mongo
            .col::<ServerPage>("server_pages")
            .find_one(revolt_database::mongodb::bson::doc! { "_id": server_id })
            .await
            .map_err(|_| create_error!(InternalError))?
            .unwrap_or_default()),
        _ => Ok(ServerPage::default()),
    }
}

#[openapi(tag = "Server Information")]
#[get("/<target>/page")]
pub async fn fetch(db: &State<Database>, target: Reference<'_>) -> Result<Json<ServerPage>> {
    let server = target.as_server(db).await?;
    Ok(Json(read(db.inner(), &server.id).await?))
}

#[openapi(tag = "Server Information")]
#[patch("/<target>/page", data = "<data>")]
pub async fn update(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<ServerPage>,
) -> Result<Json<ServerPage>> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;
    let page = data.into_inner();
    if !valid(&page) {
        return Err(create_error!(InvalidProperty));
    }
    if let Database::MongoDb(mongo) = db.inner() {
        mongo
            .col::<ServerPage>("server_pages")
            .replace_one(
                revolt_database::mongodb::bson::doc! { "_id": &server.id },
                &page,
            )
            .upsert(true)
            .await
            .map_err(|_| create_error!(InternalError))?;
    }
    Ok(Json(page))
}
