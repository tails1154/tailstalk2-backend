use revolt_database::{
    mongodb::bson::doc,
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    Database, User,
};
use revolt_permissions::{calculate_server_permissions, ChannelPermission};
use revolt_result::Result;
use rocket::{serde::json::Json, State};
use serde::Serialize;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ServerAnalytics {
    pub member_count: u64,
    pub channel_count: u64,
    pub message_count: u64,
}

/// Fetch aggregate server analytics. Requires ManageServer.
#[openapi(tag = "Server Information")]
#[get("/<target>/analytics")]
pub async fn fetch(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<ServerAnalytics>> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;

    let data = match &**db {
        Database::MongoDb(mongo) => {
            let channels = server.channels.len() as u64;
            let members = mongo
                .count_documents(
                    "server_members",
                    doc! {
                        "_id.server": &server.id,
                        "pending_deletion_at": { "$exists": false },
                    },
                )
                .await
                .unwrap_or(0);
            let messages = mongo
                .count_documents("messages", doc! { "channel": { "$in": &server.channels } })
                .await
                .unwrap_or(0);
            ServerAnalytics {
                member_count: members,
                channel_count: channels,
                message_count: messages,
            }
        }
        _ => ServerAnalytics {
            member_count: 0,
            channel_count: server.channels.len() as u64,
            message_count: 0,
        },
    };

    Ok(Json(data))
}
