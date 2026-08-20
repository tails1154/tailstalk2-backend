use revolt_database::{mongodb::bson::{self, doc, Document}, Database, Invite, PartialBot, PartialServer, User};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::routes::admin::auth::AdminUser;

const CONFIG_ID: &str = "discovery";

#[derive(Debug, Serialize, Deserialize, JsonSchema, Clone)]
pub struct DiscoveryListing {
    pub id: String,
    pub kind: String,
    pub target_id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub invite: Option<String>,
    pub members: i64,
    pub status: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmitDiscovery {
    pub invite: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
}

fn listing_from_doc(d: &Document) -> Option<DiscoveryListing> {
    Some(DiscoveryListing {
        id: d.get_str("id").ok()?.to_string(),
        kind: d.get_str("kind").unwrap_or("server").to_string(),
        target_id: d.get_str("target_id").ok()?.to_string(),
        name: d.get_str("name").unwrap_or("").to_string(),
        description: d.get_str("description").unwrap_or("").to_string(),
        category: d.get_str("category").unwrap_or("").to_string(),
        invite: d.get_str("invite").ok().map(str::to_string),
        members: d.get_i64("members").unwrap_or(0),
        status: d.get_str("status").unwrap_or("pending").to_string(),
    })
}

async fn load(db: &Database) -> Result<Vec<Document>> {
    match db {
        Database::MongoDb(mongo) => Ok(mongo
            .col::<Document>("platform_config")
            .find_one(doc! { "_id": CONFIG_ID })
            .await
            .map_err(|_| create_error!(InternalError))?
            .and_then(|d| d.get_array("entries").ok().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_document().cloned())
            .collect()),
        _ => Ok(Vec::new()),
    }
}

async fn save(db: &Database, entries: Vec<Document>) -> Result<()> {
    if let Database::MongoDb(mongo) = db {
        let entries = bson::to_bson(&entries).map_err(|_| create_error!(InternalError))?;
        mongo
            .col::<Document>("platform_config")
            .update_one(
                doc! { "_id": CONFIG_ID },
                doc! { "$set": { "entries": entries } },
            )
            .upsert(true)
            .await
            .map_err(|_| create_error!(InternalError))?;
    }
    Ok(())
}

/// List approved servers and bots for the discovery page.
#[openapi(tag = "Discovery")]
#[get("/discovery?<query>")]
pub async fn list(db: &State<Database>, query: Option<String>) -> Result<Json<Vec<DiscoveryListing>>> {
    let query = query.unwrap_or_default().to_lowercase();
    let mut entries: Vec<_> = load(db)
        .await?
        .iter()
        .filter_map(listing_from_doc)
        .filter(|entry| entry.status == "approved")
        .filter(|entry| {
            query.is_empty()
                || entry.name.to_lowercase().contains(&query)
                || entry.description.to_lowercase().contains(&query)
        })
        .collect();
    entries.sort_by(|a, b| b.members.cmp(&a.members));
    Ok(Json(entries))
}

/// Submit a server for discovery review. Servers need at least three members.
#[openapi(tag = "Discovery")]
#[post("/discovery/servers/<server_id>", data = "<data>")]
pub async fn submit_server(
    db: &State<Database>,
    user: User,
    server_id: String,
    data: Json<SubmitDiscovery>,
) -> Result<Json<DiscoveryListing>> {
    let server = db.fetch_server(&server_id).await?;
    if server.owner != user.id {
        return Err(create_error!(NotFound));
    }
    let members = db.fetch_member_count(&server.id).await? as i64;
    if members < 3 {
        return Err(create_error!(FailedValidation {
            error: "Your server needs at least 3 members to submit to discovery.".to_string()
        }));
    }
    let SubmitDiscovery { invite, description, category } = data.into_inner();
    if let Some(code) = &invite {
        match db.fetch_invite(code).await? {
            Invite::Server { server: target, .. } if target == server.id => {}
            _ => return Err(create_error!(InvalidInvite)),
        }
    }
    let entry = DiscoveryListing {
        id: Ulid::new().to_string(),
        kind: "server".to_string(),
        target_id: server.id,
        name: server.name,
        description: description.unwrap_or_default(),
        category: category.unwrap_or_default(),
        invite,
        members,
        status: "pending".to_string(),
    };
    let mut entries = load(db).await?;
    entries.retain(|d| d.get_str("target_id") != Ok(entry.target_id.as_str()));
    entries.push(bson::to_document(&entry).map_err(|_| create_error!(InternalError))?);
    save(db, entries).await?;
    Ok(Json(entry))
}

/// Submit an owned bot for discovery review.
#[openapi(tag = "Discovery")]
#[post("/discovery/bots/<bot_id>")]
pub async fn submit_bot(db: &State<Database>, user: User, bot_id: String) -> Result<Json<DiscoveryListing>> {
    let bot = db.fetch_bot(&bot_id).await?;
    if bot.owner != user.id {
        return Err(create_error!(NotFound));
    }
    let bot_user = db.fetch_user(&bot.id).await?;
    // Discovery submissions opt bots into public invitations.
    if !bot.public {
        let mut bot = bot.clone();
        bot.update(db, PartialBot { public: Some(true), ..Default::default() }, vec![])
            .await?;
    }
    let entry = DiscoveryListing {
        id: Ulid::new().to_string(),
        kind: "bot".to_string(),
        target_id: bot.id,
        name: bot_user.username,
        description: "A TailsTalk 2 bot".to_string(),
        category: "bot".to_string(),
        invite: None,
        members: 0,
        status: "pending".to_string(),
    };
    let mut entries = load(db).await?;
    entries.retain(|d| d.get_str("target_id") != Ok(entry.target_id.as_str()));
    entries.push(bson::to_document(&entry).map_err(|_| create_error!(InternalError))?);
    save(db, entries).await?;
    Ok(Json(entry))
}

#[openapi(tag = "Admin")]
#[get("/discovery")]
pub async fn admin_list(_admin: AdminUser, db: &State<Database>) -> Result<Json<Vec<DiscoveryListing>>> {
    Ok(Json(load(db).await?.iter().filter_map(listing_from_doc).collect()))
}

async fn set_status(db: &Database, id: &str, status: &str) -> Result<Json<DiscoveryListing>> {
    let mut docs = load(db).await?;
    let index = docs.iter().position(|d| d.get_str("id") == Ok(id)).ok_or(create_error!(NotFound))?;
    docs[index].insert("status", status);
    let listing = listing_from_doc(&docs[index]).ok_or(create_error!(InternalError))?;
    save(db, docs).await?;
    Ok(Json(listing))
}

#[openapi(tag = "Admin")]
#[post("/discovery/<id>/deny")]
pub async fn deny(_admin: AdminUser, db: &State<Database>, id: String) -> Result<Json<DiscoveryListing>> {
    set_status(db, &id, "denied").await
}

#[openapi(tag = "Admin")]
#[post("/discovery/<id>/approve")]
pub async fn approve(_admin: AdminUser, db: &State<Database>, id: String) -> Result<Json<DiscoveryListing>> {
    let listing = set_status(db, &id, "approved").await?.into_inner();
    if listing.kind == "server" {
        let mut server = db.fetch_server(&listing.target_id).await?;
        server.update(db, PartialServer { discoverable: Some(true), ..Default::default() }, vec![]).await?;
    } else if listing.kind == "bot" {
        let mut bot = db.fetch_bot(&listing.target_id).await?;
        bot.update(db, PartialBot { public: Some(true), discoverable: Some(true), ..Default::default() }, vec![]).await?;
    }
    Ok(Json(listing))
}
