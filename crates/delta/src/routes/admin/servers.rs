use revolt_database::{
    mongodb::bson::{self, doc},
    Database, Invite,
};
use revolt_result::Result;
use rocket::serde::json::Json;
use rocket::{form::FromForm, State};
use serde::{Deserialize, Serialize};

use super::auth::AdminUser;

#[derive(Serialize, JsonSchema, Debug)]
pub struct AdminServer {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub owner_name: String,
    pub member_count: u64,
    pub created_at: String,
    pub suspended: bool,
    pub channels: Vec<AdminServerChannel>,
    pub roles: Vec<AdminServerRole>,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct AdminServerChannel {
    pub id: String,
    pub name: String,
    pub channel_type: String,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct AdminServerRole {
    pub id: String,
    pub name: String,
}


#[derive(Deserialize, JsonSchema, Debug)]
pub struct SuspendServerData {
    pub suspended: bool,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct AdminInvite {
    pub code: String,
    pub url: String,
}

#[derive(FromForm, JsonSchema, Debug)]
pub struct ServerQuery {
    pub q: Option<String>,
}

fn text(doc: &bson::Document, key: &str) -> String {
    doc.get_str(key).unwrap_or_default().to_string()
}

async fn server_from_doc(mongo: &revolt_database::MongoDb, server: bson::Document) -> AdminServer {
    let id = text(&server, "_id");
    let owner_id = text(&server, "owner");
    let owner_name = mongo
        .col::<bson::Document>("users")
        .find_one(doc! { "_id": &owner_id })
        .await
        .ok()
        .flatten()
        .map(|user| {
            user.get_str("display_name")
                .ok()
                .filter(|name| !name.is_empty())
                .or_else(|| user.get_str("username").ok())
                .unwrap_or("Unknown user")
                .to_string()
        })
        .unwrap_or_else(|| owner_id.clone());

    let member_count = mongo
        .count_documents("server_members", doc! { "_id.server": &id })
        .await
        .unwrap_or(0);
    let mut channels = Vec::new();
    if let Ok(mut cursor) = mongo
        .col::<bson::Document>("channels")
        .find(doc! { "server": &id })
        .await
    {
        use futures::StreamExt;
        while let Some(Ok(channel)) = cursor.next().await {
            channels.push(AdminServerChannel {
                id: text(&channel, "_id"),
                name: text(&channel, "name"),
                channel_type: text(&channel, "channel_type"),
            });
        }
    }
    let roles = server
        .get_document("roles")
        .ok()
        .map(|roles| {
            roles
                .iter()
                .map(|(id, role)| AdminServerRole {
                    id: id.clone(),
                    name: role
                        .as_document()
                        .map(|r| text(r, "name"))
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();

    AdminServer {
        id,
        name: text(&server, "name"),
        owner_id,
        owner_name,
        member_count,
        created_at: text(&server, "created_at"),
        suspended: server.get_bool("admin_suspended").unwrap_or(false),
        channels,
        roles,
    }
}

#[openapi(tag = "Admin")]
#[get("/servers?<query..>")]
pub async fn admin_servers(
    _admin: AdminUser,
    db: &State<Database>,
    query: ServerQuery,
) -> Result<Json<Vec<AdminServer>>> {
    let Database::MongoDb(mongo) = db.inner() else {
        return Ok(Json(Vec::new()));
    };
    let needle = query.q.unwrap_or_default().to_lowercase();
    let mut cursor = mongo
        .col::<bson::Document>("servers")
        .find(doc! {})
        .await
        .map_err(|_| revolt_result::create_error!(InternalError))?;
    use futures::StreamExt;
    let mut result = Vec::new();
    while let Some(Ok(server)) = cursor.next().await {
        let name = text(&server, "name");
        if !needle.is_empty()
            && !name.to_lowercase().contains(&needle)
            && !text(&server, "_id").contains(&needle)
        {
            continue;
        }
        result.push(server_from_doc(mongo, server).await);
    }
    Ok(Json(result))
}

#[openapi(tag = "Admin")]
#[post("/servers/<id>/suspend", data = "<data>")]
pub async fn admin_suspend_server(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
    data: Json<SuspendServerData>,
) -> Result<Json<serde_json::Value>> {
    let Database::MongoDb(mongo) = db.inner() else {
        return Ok(Json(serde_json::json!({"ok": true})));
    };
    mongo
        .col::<bson::Document>("servers")
        .update_one(
            doc! { "_id": &id },
            doc! { "$set": { "admin_suspended": data.suspended } },
        )
        .await
        .map_err(|_| revolt_result::create_error!(InternalError))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[openapi(tag = "Admin")]
#[delete("/servers/<id>")]
pub async fn admin_delete_server(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
) -> Result<Json<serde_json::Value>> {
    let Database::MongoDb(mongo) = db.inner() else {
        return Ok(Json(serde_json::json!({"ok": true})));
    };
    revolt_database::AbstractServers::delete_server(mongo, &id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[openapi(tag = "Admin")]
#[post("/servers/<id>/invite")]
pub async fn admin_create_server_invite(
    _admin: AdminUser,
    db: &State<Database>,
    id: String,
) -> Result<Json<AdminInvite>> {
    let server = db.fetch_server(&id).await?;
    let owner = db.fetch_user(&server.owner).await?;
    let channel_id = server
        .channels
        .first()
        .ok_or_else(|| revolt_result::create_error!(NotFound))?;
    let channel = db.fetch_channel(channel_id).await?;
    let invite = Invite::create_channel_invite(db, &owner, &channel).await?;
    let code = invite.code().to_string();
    Ok(Json(AdminInvite {
        code: code.clone(),
        url: format!("https://tailstalk2.com/{code}"),
    }))
}
