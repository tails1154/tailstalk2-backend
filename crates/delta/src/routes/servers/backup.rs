use std::collections::HashMap;

use revolt_database::{
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    Database, PartialServer, Role, User,
};
use revolt_permissions::{calculate_server_permissions, ChannelPermission};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ServerBackup {
    pub version: u8,
    pub server_id: String,
    pub name: String,
    pub description: Option<String>,
    #[schemars(skip)]
    pub roles: HashMap<String, Role>,
    #[schemars(skip)]
    pub categories: Option<Vec<revolt_database::Category>>,
    pub default_permissions: i64,
}

#[openapi(tag = "Server Information")]
#[get("/<target>/backup")]
pub async fn export(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<ServerBackup>> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;
    Ok(Json(ServerBackup {
        version: 1,
        server_id: server.id,
        name: server.name,
        description: server.description,
        roles: server.roles,
        categories: server.categories,
        default_permissions: server.default_permissions,
    }))
}

#[openapi(tag = "Server Information")]
#[post("/<target>/backup", data = "<data>")]
pub async fn restore(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<ServerBackup>,
) -> Result<Json<ServerBackup>> {
    let mut server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;
    let backup = data.into_inner();
    if backup.version != 1 || backup.server_id != server.id {
        return Err(create_error!(InvalidProperty));
    }
    let partial = PartialServer {
        name: Some(backup.name.clone()),
        description: backup.description.clone(),
        roles: Some(backup.roles.clone()),
        categories: backup.categories.clone(),
        default_permissions: Some(backup.default_permissions),
        ..Default::default()
    };
    server.update(db, partial, vec![]).await?;
    Ok(Json(backup))
}
