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

fn validate_backup(backup: &ServerBackup, server_id: &str) -> Result<()> {
    if backup.version != 1 || backup.server_id != server_id || backup.name.trim().is_empty() {
        return Err(create_error!(InvalidProperty));
    }
    Ok(())
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
    validate_backup(&backup, &server.id)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test::TestHarness;
    use revolt_models::v0::DataCreateServer;
    use rocket::http::{ContentType, Header, Status};

    #[test]
    fn rejects_wrong_version_server_and_empty_name() {
        let backup = ServerBackup {
            version: 2,
            server_id: "server".into(),
            name: "Name".into(),
            description: None,
            roles: HashMap::new(),
            categories: None,
            default_permissions: 0,
        };
        assert!(validate_backup(&backup, "server").is_err());

        let mut backup = backup;
        backup.version = 1;
        backup.server_id = "other".into();
        assert!(validate_backup(&backup, "server").is_err());

        backup.server_id = "server".into();
        backup.name = "   ".into();
        assert!(validate_backup(&backup, "server").is_err());
    }

    #[ignore = "requires the MongoDB test service"]
    #[rocket::async_test]
    async fn export_then_restore_round_trip() {
        let harness = TestHarness::new().await;
        let (_, session, user) = harness.new_user().await;
        let (server, _) = revolt_database::Server::create(
            &harness.db,
            DataCreateServer {
                name: TestHarness::rand_string(),
                ..Default::default()
            },
            &user,
            false,
        )
        .await
        .unwrap();

        let token = Header::new("x-session-token", session.token.to_string());
        let response = harness
            .client
            .get(format!("/servers/{}/backup", server.id))
            .header(token.clone())
            .dispatch()
            .await;
        assert_eq!(response.status(), Status::Ok);
        let backup = response.into_json::<ServerBackup>().await.unwrap();

        let response = harness
            .client
            .post(format!("/servers/{}/backup", server.id))
            .header(ContentType::JSON)
            .header(token)
            .body(serde_json::to_string(&backup).unwrap())
            .dispatch()
            .await;
        assert_eq!(response.status(), Status::Ok);
    }
}
