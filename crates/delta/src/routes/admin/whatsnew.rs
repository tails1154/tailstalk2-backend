use revolt_database::{
    mongodb::bson::{self, doc},
    Database,
};
use revolt_result::Result;
use rocket::State;
use rocket::serde::json::Json;
use serde::{Deserialize, Serialize};

use super::auth::AdminUser;

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
pub struct WhatsNewEntry {
    pub id: String,
    pub title: String,
    pub body: String,
    pub date: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug)]
pub struct WhatsNewData {
    pub entries: Vec<WhatsNewEntry>,
}

/// # Get What's New
///
/// Get the current What's New content.
#[openapi(tag = "Admin")]
#[get("/whatsnew")]
pub async fn admin_get_whatsnew(
    _admin: AdminUser,
    db: &State<Database>,
) -> Result<Json<WhatsNewData>> {
    let entries = match db.inner() {
        Database::MongoDb(mongo) => {
            if let Ok(Some(doc)) = mongo
                .col::<bson::Document>("platform_config")
                .find_one(doc! { "_id": "whatsnew" })
                .await
            {
                doc.get_array("entries")
                    .map(|arr| {
                        arr.iter()
                            .enumerate()
                            .filter_map(|(index, v)| {
                                let d = v.as_document()?;
                                Some(WhatsNewEntry {
                                    id: d
                                        .get_str("id")
                                        .map(str::to_string)
                                        .unwrap_or_else(|_| format!("legacy-{index}")),
                                    title: d.get_str("title").unwrap_or("").to_string(),
                                    body: d.get_str("body").unwrap_or("").to_string(),
                                    date: d.get_str("date").unwrap_or("").to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };

    Ok(Json(WhatsNewData { entries }))
}

/// # Set What's New
///
/// Update the What's New content.
#[openapi(tag = "Admin")]
#[post("/whatsnew", data = "<data>")]
pub async fn admin_set_whatsnew(
    _admin: AdminUser,
    db: &State<Database>,
    data: Json<WhatsNewData>,
) -> Result<Json<serde_json::Value>> {
    let entries_bson: Vec<bson::Bson> = data
        .entries
        .iter()
        .map(|e| {
            let mut document = bson::to_document(e).unwrap_or_default();
            if document
                .get_str("id")
                .map(|id| id.is_empty())
                .unwrap_or(true)
            {
                document.insert("id", ulid::Ulid::new().to_string());
            }
            bson::Bson::Document(document)
        })
        .collect();

    match db.inner() {
        Database::MongoDb(mongo) => {
            mongo
                .col::<bson::Document>("platform_config")
                .update_one(
                    doc! { "_id": "whatsnew" },
                    doc! {
                        "$set": {
                            "entries": bson::to_bson(&entries_bson).unwrap_or_default(),
                        }
                    },
                )
                .upsert(true)
                .await
                .map_err(|_| revolt_result::create_error!(InternalError))?;
        }
        _ => {}
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}
