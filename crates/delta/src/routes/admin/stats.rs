use revolt_database::{mongodb::bson::doc, Database};
use revolt_result::Result;
use rocket::State;
use rocket::serde::json::Json;
use serde::Serialize;

use super::auth::AdminUser;

#[derive(Serialize, JsonSchema, Debug)]
pub struct AdminStats {
    pub users: u64,
    pub servers: u64,
    pub channels: u64,
    pub messages: u64,
    pub bots: u64,
    pub reports: u64,
}

async fn count_collection(db: &Database, collection: &'static str) -> u64 {
    match db {
        Database::MongoDb(mongo) => {
            mongo.count_documents(collection, doc! {}).await.unwrap_or(0)
        }
        _ => 0,
    }
}

/// # Admin Stats
///
/// Fetch admin dashboard stats. Requires HTTP Basic Auth.
#[openapi(tag = "Admin")]
#[get("/")]
pub async fn admin_stats(
    _admin: AdminUser,
    db: &State<Database>,
) -> Result<Json<AdminStats>> {
    let data = db.inner().clone();

    let (users, servers, channels, messages, bots, reports) = tokio::join!(
        count_collection(&data, "users"),
        count_collection(&data, "servers"),
        count_collection(&data, "channels"),
        count_collection(&data, "messages"),
        count_collection(&data, "bots"),
        count_collection(&data, "safety_reports"),
    );

    Ok(Json(AdminStats {
        users,
        servers,
        channels,
        messages,
        bots,
        reports,
    }))
}
